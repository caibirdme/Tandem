#!/usr/bin/env python3
"""Exercise real remote stdio, shared Unix proxies, and isolated OpenSSH.

The SDK and tools are production code; inference and OTLP use the same local
fixtures as runtime_e2e.py. SSH uses ephemeral keys, an isolated known-hosts file,
and an unprivileged localhost sshd. A forced command confines the test server's
home and runtime state to a temporary directory. No real provider key is needed.

Run after building tandem-server and tandem-dev-server: python3 tests/remote_e2e.py
Use --server for the remote stdio executable and --dev-server for the HTTP host.
With --skip-ssh, only the remote executable is required. Unix hosts also verify
shared daemon/proxy sessions without requiring an SSH daemon.
"""

from __future__ import annotations

import argparse
import getpass
import hashlib
import json
import os
import queue
import shlex
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
from http.server import ThreadingHTTPServer
from pathlib import Path

from acceptance_support import Application, activity_tools, debug_binary, require_context_usage
from activity_acceptance import check_grouped_activity
from fleet_acceptance import check_fleet, check_stdio_fleet
from credential_checks import require_local_secret, require_provider_key, scan_for_key
from inference_fixture import (
    FIXTURE_KEY,
    Fixture,
    handler_for,
    json_span_names,
    require,
)
from permission_acceptance import check_permission_modes
from model_switch_acceptance import require_wire_history
from reasoning_acceptance import (
    check_pending_reasoning_change,
    check_reasoning_turn,
    check_thinking_text,
    require_runtime_thinking,
    require_wire_effort,
    set_effort,
    wire_effort,
)
from skill_acceptance import check_project_skills, check_stdio_skills
from attachment_acceptance import check_attachments, check_stdio_attachments
from remote_protocol import (
    StdioServer,
    child_processes,
    private_environment,
)
from daemon_acceptance import check_daemon, require_runtime, runtime_processes, stop_daemon


def require_no_persisted_key(home):
    leaks = scan_for_key(home, FIXTURE_KEY)
    require(not leaks, f'Provider credential persisted on remote host: {leaks}')


def session_options(session_id, endpoint):
    return {
        'sessionId': session_id,
        'mode': {'kind': 'chat'},
        'permissionMode': 'ask',
        'provider': {'protocol': 'openai-completions', 'baseUrl': endpoint + '/v1', 'apiKey': FIXTURE_KEY},
        'model': {'id': 'gpt-4.1', 'contextWindow': 128000, 'maxOutputTokens': 4096},
        'systemPrompt': 'Use the requested fixture response.',
    }


def check_stdio(binary, home, endpoint, fixture):
    home.mkdir()
    initialized = subprocess.run([str(binary), '--install-runtime'], capture_output=True,
                                 env=private_environment(home), timeout=45, check=False)
    require(initialized.returncode == 0, 'Bundled remote runtime initialization failed')
    if os.name == 'posix':
        bootstrap = StdioServer(binary, home, mode='--proxy')
        try:
            daemon = bootstrap.command('server_info')['processId']
        finally:
            bootstrap.close()
        stop_daemon(daemon)
    session_id = str(uuid.uuid4())
    options = session_options(session_id, endpoint)
    telemetry = {'enabled': True, 'endpoint': endpoint, 'protocol': 'http/json', 'captureContent': False}
    first_prompt = 'fixture:model-switch Keep STDIO_ORIGINAL_HISTORY before a cold reconnect.'
    first_response = 'TANDEM_MODEL_SWITCH_OK ' + options['model']['id']
    server = StdioServer(binary, home)
    try:
        try:
            server.command('unknown_remote_command')
            raise AssertionError('Unknown remote command was accepted')
        except RuntimeError as error:
            require('Unsupported remote command' in str(error), 'Wrong remote command error')
        server.command('start_agent', options=options, resume=False, telemetry=telemetry)
        events = server.turn(session_id, first_prompt)
        require_context_usage(events, 128000)
        require(any(event['kind'] == 'delta' for event in events), 'stdio streaming delta missing')
        require(any(event['kind'] == 'message' and event['data']['content'] == first_response
                    for event in events), 'stdio final response missing')
        check_stdio_skills(server, fixture, options, home)
        check_stdio_attachments(server, fixture, options)
        check_stdio_fleet(server, fixture, options)
        for protocol, model in (('openai-responses', 'gpt-4.1'),
                                ('anthropic', 'claude-sonnet-4.5')):
            attachment_options = {
                **options,
                'provider': {**options['provider'], 'protocol': protocol,
                             'baseUrl': endpoint if protocol == 'anthropic' else endpoint + '/v1'},
                'model': {**options['model'], 'id': model},
            }
            check_stdio_attachments(server, fixture, attachment_options)
        check_stdio_reasoning(server, fixture, endpoint)
    finally:
        server.close()
    resumed = StdioServer(binary, home)
    try:
        options['provider']['protocol'] = 'anthropic'
        options['provider']['baseUrl'] = endpoint
        options['model']['id'] = 'claude-sonnet-4.5'
        offset = len(fixture.conversation_requests())
        connected = resumed.command('start_agent', options=options, resume=True, telemetry=telemetry)
        require(connected['sessionId'] == session_id, 'Cold model change created a different remote session')
        prompt = 'fixture:model-switch Continue STDIO_NEW_MODEL with the prior conversation.'
        events = resumed.turn(session_id, prompt)
        require_context_usage(events, 128000)
        require(any(event['kind'] == 'message' and event['data']['content'] ==
                    'TANDEM_MODEL_SWITCH_OK ' + options['model']['id'] for event in events),
                'Resumed remote model did not return the expected response')
        requests = fixture.conversation_requests()[offset:]
        require(bool(requests), 'Resumed remote model never reached inference')
        for path, body in requests:
            require(path.split('?', 1)[0] == '/v1/messages',
                    'Remote model change reused the previous provider wire protocol')
            require(body['model'] == options['model']['id'], 'Remote cold resume reused the previous model')
            require(prompt in json.dumps(body), 'Remote cold resume replayed an earlier prompt')
            require_wire_history(body, [
                {'role': 'user', 'content': first_prompt},
                {'role': 'assistant', 'content': first_response},
            ])
        resumed.command('disconnect_agent', sessionId=session_id)
    finally:
        resumed.close()
    # Invalid input must still stop a runtime that has already been initialized.
    for malformed in ('{invalid-json}\n', 'x' * (32 * 1024 * 1024 + 1) + '\n'):
        invalid = StdioServer(binary, home)
        invalid.command('start_agent', options=session_options(str(uuid.uuid4()), endpoint), resume=False)
        try:
            invalid.process.stdin.write(malformed)
            invalid.process.stdin.flush()
        except BrokenPipeError:
            pass
        invalid.close(expected_status=1)
    require(any(path == '/v1/traces' and 'json' in content_type and json_span_names(body)
                for path, body, content_type in fixture.exports), 'Remote EOF did not flush valid OTLP spans')
    require_no_persisted_key(home)


def check_stdio_reasoning(server, fixture, endpoint):
    options = session_options(str(uuid.uuid4()), endpoint)
    options['model']['id'] = 'gpt-5'
    transcript = []
    native_effort = None
    for index, effort in enumerate((None, 'low', 'high', 'max', None)):
        options['model']['reasoningEffort'] = effort
        server.command('start_agent', options=options, resume=bool(transcript))
        prompt = f'fixture:chat Preserve STDIO_REASONING_HISTORY with effort {effort}.'
        offset = len(fixture.conversation_requests())
        events = server.turn(options['sessionId'], prompt)
        requests = fixture.conversation_requests()[offset:]
        require(bool(requests), 'Remote reasoning selection never reached inference')
        if index == 0:
            native_effort = wire_effort(*requests[0])
        for path, body in requests:
            require_wire_effort(path, body, effort if effort is not None else native_effort)
            require_wire_history(body, transcript)
        response = next(event['data']['content'] for event in events
                        if event['kind'] == 'message')
        transcript.extend(({'role': 'user', 'content': prompt},
                           {'role': 'assistant', 'content': response}))
        server.command('disconnect_agent', sessionId=options['sessionId'])
    thinking = session_options(str(uuid.uuid4()), endpoint)
    thinking['provider'].update(protocol='anthropic', baseUrl=endpoint)
    thinking['model'].update(id='claude-sonnet-4.5', reasoningEffort='high')
    server.command('start_agent', options=thinking, resume=False)
    events = server.turn(thinking['sessionId'], 'fixture:thinking Expose native thinking through remote stdio.')
    require_runtime_thinking(events)
    require(any(event['kind'] == 'message' and event['data'].get('content') == 'TANDEM_THINKING_OK'
                for event in events), 'Remote thinking contaminated the final assistant answer')
    server.command('disconnect_agent', sessionId=thinking['sessionId'])


class IsolatedSsh:
    def __init__(self, directory):
        self.directory = directory
        self.runtime_daemon = None
        self.runtime_daemons = set()
        directory.mkdir()
        self.home = directory / 'remote-home'
        self.home.mkdir()
        self.command_marker = directory / 'ssh-commands'
        self.command_marker.write_bytes(b'')
        self.known_hosts = directory / 'known_hosts'
        self.known_hosts.write_text('')
        ssh = shutil.which('ssh')
        sshd = shutil.which('sshd') or ('/usr/sbin/sshd' if Path('/usr/sbin/sshd').is_file() else None)
        keygen = shutil.which('ssh-keygen')
        require(ssh and sshd and keygen, 'OpenSSH client, daemon, and ssh-keygen are required for SSH acceptance')
        for name in ('host', 'identity', 'wrong-identity'):
            subprocess.run([keygen, '-q', '-t', 'ed25519', '-N', '', '-f', str(directory / name)], check=True)
        with socket.socket() as probe:
            probe.bind(('127.0.0.1', 0))
            self.port = probe.getsockname()[1]
        forced = directory / 'remote-shell.py'
        remote_environment = {'HOME': str(self.home), 'XDG_CACHE_HOME': str(self.home / '.cache'),
                              'XDG_CONFIG_HOME': str(self.home / '.config'), 'XDG_STATE_HOME': str(self.home / '.state')}
        forced.write_text('import os, subprocess, sys, time\nfrom pathlib import Path\n'
                          f'with open({str(self.command_marker)!r}, "ab", buffering=0) as marker:\n'
                          '    marker.write(b"x")\n'
                          f'os.environ.update({remote_environment!r})\n'
                          f'os.chdir({str(self.home)!r})\n'
                          'delay = Path("delay-bootstrap")\n'
                          'if "uname -s" in os.environ["SSH_ORIGINAL_COMMAND"] and delay.exists():\n'
                          '    Path("bootstrap-waiting").touch()\n'
                          '    while delay.exists(): time.sleep(.05)\n'
                          'race = Path("race-bootstrap")\n'
                          'command = os.environ["SSH_ORIGINAL_COMMAND"]\n'
                          'if race.exists() and "uname -s" in command:\n'
                          '    result = subprocess.run(["/bin/sh", "-c", command], capture_output=True)\n'
                          '    (race / f"probe-{os.getpid()}").touch()\n'
                          '    while not (race / "proceed").exists(): time.sleep(.02)\n'
                          '    sys.stdout.buffer.write(result.stdout)\n'
                          '    sys.stderr.buffer.write(result.stderr)\n'
                          '    sys.exit(result.returncode)\n'
                          'if race.exists() and "destination=" in command:\n'
                          '    try:\n'
                          '        (race / "first-upload").mkdir()\n'
                          '    except FileExistsError:\n'
                          '        (race / "second-upload").touch()\n'
                          '        while not (race / "release-second").exists(): time.sleep(.02)\n'
                          'os.execv("/bin/sh", ["sh", "-c", os.environ["SSH_ORIGINAL_COMMAND"]])\n')
        config = directory / 'sshd_config'
        config.write_text('\n'.join([
            f'Port {self.port}', 'ListenAddress 127.0.0.1', f'HostKey {directory}/host',
            f'PidFile {directory}/sshd.pid', f'AuthorizedKeysFile {directory}/identity.pub',
            'PasswordAuthentication no', 'KbdInteractiveAuthentication no', 'UsePAM no',
            'StrictModes no', 'PermitUserRC no', 'AllowTcpForwarding local', 'AllowAgentForwarding no',
            f'PermitOpen 127.0.0.1:{self.port}',
            f'AllowUsers {getpass.getuser()}', 'LogLevel ERROR',
            f'ForceCommand {shlex.quote(sys.executable)} {shlex.quote(str(forced))}', '',
        ]))
        self.log = open(directory / 'sshd.log', 'w+')
        self.daemon = subprocess.Popen([sshd, '-D', '-e', '-f', str(config)], stdout=self.log,
                                       stderr=self.log, start_new_session=True)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            require(self.daemon.poll() is None, 'Isolated SSH daemon exited during startup')
            try:
                with socket.create_connection(('127.0.0.1', self.port), timeout=.2):
                    break
            except OSError:
                time.sleep(.05)
        else:
            raise TimeoutError('Isolated SSH daemon did not become ready')
        wrapper_directory = directory / 'bin'
        wrapper_directory.mkdir()
        self.client_home = directory / 'client-home'
        config_directory = self.client_home / '.ssh'
        config_directory.mkdir(parents=True, mode=0o700)
        self.client_config = config_directory / 'config'
        self.client_config.write_text('Include hosts/*.conf\n')
        self.client_config.chmod(0o600)
        hosts = config_directory / 'hosts'
        hosts.mkdir()
        (hosts / 'acceptance.conf').write_text('\n'.join([
            'Host tandem-config-target', '    HostName 127.0.0.1',
            '    ProxyJump tandem-config-jump',
            'Host tandem-config-target tandem-config-jump',
            '    HostName 127.0.0.1', f'    Port {self.port}',
            f'    User {getpass.getuser()}', f'    IdentityFile "{directory / "identity"}"',
            '    IdentitiesOnly yes', '    StrictHostKeyChecking yes',
            f'    UserKnownHostsFile "{self.known_hosts}"', '    GlobalKnownHostsFile /dev/null', '',
        ]))
        (hosts / 'acceptance.conf').chmod(0o600)
        wrapper = wrapper_directory / 'ssh'
        wrapper.write_text('#!/bin/sh\nexec ' + shlex.join([
            ssh, '-F', str(self.client_config), '-o', f'UserKnownHostsFile={self.known_hosts}',
            '-o', 'GlobalKnownHostsFile=/dev/null', '-o', 'IdentitiesOnly=yes',
        ]) + ' "$@"\n')
        wrapper.chmod(0o700)
        self.environment = os.environ.copy()
        self.environment['PATH'] = str(wrapper_directory) + os.pathsep + self.environment.get('PATH', '')
        self.environment['HOME'] = str(self.client_home)
        self.environment['USERPROFILE'] = str(self.client_home)
        self.connection = {'kind': 'ssh', 'host': '127.0.0.1', 'port': self.port,
                           'user': getpass.getuser(), 'identityFile': str(directory / 'identity')}

    def command_count(self):
        """Count remote launches without recording commands or private payloads."""
        return self.command_marker.stat().st_size

    def trust_fixture_key(self):
        key_type, key, *_ = (self.directory / 'host.pub').read_text().split()
        self.known_hosts.write_text(f'[127.0.0.1]:{self.port} {key_type} {key}\n')

    def identify_runtime_daemon(self):
        arguments = [str(self.directory / 'bin/ssh'), '-T', '-p', str(self.port),
                     '-i', str(self.directory / 'identity'), '-o', 'BatchMode=yes',
                     f'{getpass.getuser()}@127.0.0.1',
                     'exec "$HOME/.local/share/tandem/bin/tandem-server" --proxy']
        client = StdioServer(None, self.home, mode='--proxy', arguments=arguments,
                             environment=self.environment)
        try:
            self.runtime_daemon = client.command('server_info')['processId']
            self.runtime_daemons.add(self.runtime_daemon)
        finally:
            client.close()

    def close(self):
        try:
            for daemon in self.runtime_daemons:
                stop_daemon(daemon)
        finally:
            if self.daemon.poll() is None:
                os.killpg(self.daemon.pid, signal.SIGTERM)
                try:
                    self.daemon.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(self.daemon.pid, signal.SIGKILL)
                    self.daemon.wait(timeout=5)
            self.log.close()


def digest(path):
    result = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            result.update(block)
    return result.digest()


def expect_command_error(app, command, expected, **payload):
    try:
        app.command(command, **payload)
    except RuntimeError as error:
        require(expected in str(error), f'{command} returned the wrong failure category')
    else:
        raise AssertionError(f'{command} unexpectedly succeeded')


def check_concurrent_bootstrap(app, ssh):
    """Force two fresh probes to deploy the same build across a live daemon."""
    race = ssh.home / 'race-bootstrap'
    race.mkdir()
    results = queue.Queue()

    def bootstrap():
        try:
            app.command('check_connection', connection=ssh.connection)
            results.put(None)
        except Exception as error:
            results.put(error)

    starters = [threading.Thread(target=bootstrap, daemon=True) for _ in range(2)]
    for starter in starters:
        starter.start()
    try:
        deadline = time.monotonic() + 15
        while len(list(race.glob('probe-*'))) < 2 and time.monotonic() < deadline:
            time.sleep(.02)
        require(len(list(race.glob('probe-*'))) == 2, 'Concurrent SSH bootstrap did not perform two fresh probes')
        (race / 'proceed').touch()
        first = results.get(timeout=45)
        if first:
            raise first
        deadline = time.monotonic() + 15
        while not (race / 'second-upload').exists() and time.monotonic() < deadline:
            time.sleep(.02)
        require((race / 'second-upload').exists(), 'The second SSH deployment did not reach its controlled pause')
        ssh.identify_runtime_daemon()
        daemon = ssh.runtime_daemon
        runtime = require_runtime(daemon)
        (race / 'release-second').touch()
        second = results.get(timeout=45)
        if second:
            raise second
        ssh.identify_runtime_daemon()
        require(ssh.runtime_daemon == daemon, 'Identical concurrent deployments created different remote daemons')
        require_runtime(daemon, runtime)
    finally:
        (race / 'proceed').touch()
        (race / 'release-second').touch()
        for starter in starters:
            starter.join(timeout=45)
        require(not any(starter.is_alive() for starter in starters), 'Concurrent SSH bootstrap did not finish')
        shutil.rmtree(race)


def check_independent_stop(app, work, second, ssh, fixture):
    runtime = require_runtime(ssh.runtime_daemon)
    fixture.release_slow_response.clear()
    app.command('send_message', sessionId=work['id'], content='fixture:slow Stream until stopped.',
                messageId=str(uuid.uuid4()))
    app.wait_event(work['id'], lambda event: event['kind'] == 'delta')
    live = app.command('snapshot')['activeTurns'][work['id']]
    require(bool(live['stream']), 'Snapshot did not preserve current streamed output')
    delay = ssh.home / 'delay-bootstrap'
    waiting = ssh.home / 'bootstrap-waiting'
    delay.touch()
    results = queue.Queue()

    def start_second():
        try:
            app.command('send_message', sessionId=second['id'], content='fixture:chat A second SSH session.',
                        messageId=str(uuid.uuid4()))
            results.put(None)
        except Exception as error:
            results.put(error)

    starter = threading.Thread(target=start_second, daemon=True)
    starter.start()
    deadline = time.monotonic() + 10
    while not waiting.exists() and time.monotonic() < deadline:
        time.sleep(.05)
    stopped = queue.Queue()

    def stop_first():
        try:
            app.command('abort_session', sessionId=work['id'])
            stopped.put(None)
        except Exception as error:
            stopped.put(error)

    stopper = threading.Thread(target=stop_first, daemon=True)
    try:
        require(waiting.exists(), 'Second session did not reach the controlled bootstrap pause')
        stopper.start()
        try:
            result = stopped.get(timeout=3)
        except queue.Empty:
            raise AssertionError('One SSH bootstrap blocked stopping an unrelated active session') from None
        if result:
            raise result
    finally:
        delay.unlink(missing_ok=True)
        fixture.release_slow_response.set()
        starter.join(timeout=30)
        if stopper.ident:
            stopper.join(timeout=10)
    result = results.get(timeout=5)
    if result:
        raise result
    app.wait_event(second['id'], lambda event: event['kind'] == 'status' and event['data']['status'] == 'idle')
    require_runtime(ssh.runtime_daemon, runtime)
    transports = [process for process in child_processes(app.process.pid)
                  if Path(subprocess.run(['ps', '-p', str(process), '-o', 'comm='],
                                         capture_output=True, text=True, check=False).stdout.strip()).name == 'ssh']
    require(len(transports) == 2,
            f'Two remote conversations must retain two independent SSH proxies; found {len(transports)}')
    messages = app.command('messages', sessionId=work['id'])
    require(any(message.get('interrupted') and message['content'] for message in messages),
            'Stopped streamed output was not persisted as an interrupted message')
    app.command('delete_session', sessionId=second['id'])


def check_remote_runtime_restart(app, work, ssh, fixture):
    fixture.release_slow_response.clear()
    app.command('send_message', sessionId=work['id'], content='fixture:slow Wait while the remote runtime exits.',
                messageId=str(uuid.uuid4()))
    app.wait_event(work['id'], lambda event: event['kind'] == 'delta')
    runtimes = runtime_processes(ssh.runtime_daemon)
    require(len(runtimes) == 1, 'Could not identify this test remote daemon\'s sole SDK child')
    os.kill(runtimes[0], signal.SIGKILL)
    fixture.release_slow_response.set()
    app.wait_event(work['id'], lambda event: event['kind'] == 'error')
    requests_before = len(fixture.conversation_requests())
    app.turn(work, 'fixture:chat Recover the remote runtime on this explicit new message.')
    require(len(fixture.conversation_requests()) == requests_before + 1, 'Remote runtime recovery replayed the interrupted prompt')
    ssh.identify_runtime_daemon()
    require_runtime(ssh.runtime_daemon)


def check_ssh_reasoning(app, fixture, directory, endpoint, project):
    provider = app.command('save_provider', provider={
        'id': '', 'name': 'SSH reasoning acceptance', 'protocol': 'openai-completions',
        'baseUrl': endpoint + '/v1',
        'models': [{'id': 'gpt-5', 'name': 'Reasoning fixture', 'reasoningEffort': None,
                    'contextWindow': 128000, 'maxOutputTokens': 4096}],
    }, apiKey=FIXTURE_KEY)
    work = app.command('create_session', projectId=project['id'], providerId=provider['id'], modelId='gpt-5')
    offset = len(fixture.conversation_requests())
    events = app.turn(work, 'fixture:chat Preserve SSH_REASONING_ORIGINAL_HISTORY with the native default.')
    app.wait_title(work['id'], events)
    requests = fixture.conversation_requests()[offset:]
    require(bool(requests), 'SSH native reasoning baseline never reached inference')
    native_effort = wire_effort(*requests[0])
    for path, body in requests:
        require_wire_effort(path, body, native_effort)
    for effort in ('high', 'max', 'low'):
        work = set_effort(app, directory, work, effort)
        check_reasoning_turn(app, fixture, work,
                             f'fixture:chat Apply SSH reasoning override {effort} in the original conversation.', effort)
    work = set_effort(app, directory, work, 'medium')
    work = check_pending_reasoning_change(app, fixture, directory, work)
    work = set_effort(app, directory, work, None)
    check_reasoning_turn(app, fixture, work,
                         'fixture:chat Reset SSH reasoning to the original native default.', native_effort)
    app.command('delete_session', sessionId=work['id'])
    thinking_provider = app.command('save_provider', provider={
        'id': '', 'name': 'SSH thinking text acceptance', 'protocol': 'anthropic',
        'baseUrl': endpoint,
        'models': [{'id': 'claude-sonnet-4.5', 'name': 'Thinking fixture',
                    'contextWindow': 128000, 'maxOutputTokens': 4096}],
    }, apiKey=FIXTURE_KEY)
    thinking = check_thinking_text(app, fixture, thinking_provider, 'claude-sonnet-4.5', project['id'])
    app.command('delete_session', sessionId=thinking['session']['id'])


def check_ssh(remote_server, dev_server, root, endpoint, fixture):
    ssh = IsolatedSsh(root / 'ssh')
    app_directory = root / 'desktop'
    app_directory.mkdir()
    app = None
    exports_before = len(fixture.exports)
    try:
        environment = ssh.environment.copy()
        # Use the staged sibling tandem-server, not an ambient artifact override.
        environment.pop('TANDEM_REMOTE_ARTIFACTS', None)
        app = Application(dev_server, app_directory, env=environment)
        require(app.command('list_ssh_hosts') == ['tandem-config-jump', 'tandem-config-target'],
                'SSH config discovery lost included aliases or retained duplicates')
        expect_command_error(app, 'check_connection', 'host key', connection=ssh.connection)
        require(ssh.known_hosts.read_text() == '', 'Unknown SSH host was silently trusted')
        ssh.trust_fixture_key()
        trusted_before = ssh.known_hosts.read_bytes()
        wrong_key = {**ssh.connection, 'identityFile': str(ssh.directory / 'wrong-identity')}
        expect_command_error(app, 'check_connection', 'authentication', connection=wrong_key)
        check_concurrent_bootstrap(app, ssh)
        deployed = ssh.home / '.local/share/tandem/bin/tandem-server'
        require(deployed.is_file(), 'SSH bootstrap did not deploy tandem-server')
        require(deployed.stat().st_mode & 0o777 == 0o700, 'Deployed server is not private and executable')
        require(digest(deployed) == digest(remote_server), 'SSH deployed bytes differ from tandem-server (the remote artifact)')
        modification = deployed.stat().st_mtime_ns
        app.command('check_connection', connection=ssh.connection)
        require(deployed.stat().st_mtime_ns == modification, 'Matching server version was needlessly replaced')
        previous_protocol = deployed.with_name('previous-protocol')
        previous_protocol.write_text('#!/bin/sh\n[ "$1" = "--version" ] || exit 97\nprintf "tandem-server 0.0.3\\n"\n')
        previous_protocol.chmod(0o700)
        previous_protocol.replace(deployed)
        app.command('check_connection', connection=ssh.connection)
        require(digest(deployed) == digest(remote_server),
                'A server without the current protocol version was reused')
        ssh.identify_runtime_daemon()
        configured_connection = {'kind': 'ssh', 'host': 'tandem-config-target',
                                 'port': None, 'user': None, 'identityFile': None}
        app.command('check_connection', connection=configured_connection)
        workspace = ssh.home / "project ' $(exit 91)"
        workspace.mkdir()
        fixture.workspace = workspace
        listing = app.command('browse_directory', connection=ssh.connection, path='~')
        require(any(entry['name'] == workspace.name for entry in listing['entries']), 'SSH directory browsing lost literal path')
        configured_listing = app.command('browse_directory', connection=configured_connection, path='~')
        require(configured_listing == listing,
                'SSH alias did not inherit the configured port, username, identity, and ProxyJump')
        configured_project = app.command('create_project', name='Configured SSH host', path=str(workspace),
                                         connection=configured_connection)
        require(configured_project['connection'] == configured_connection,
                'Saving an SSH alias replaced inherited values with overrides')
        configured_saved = next(project for project in app.command('snapshot')['projects']
                                if project['id'] == configured_project['id'])
        require(configured_saved['connection'] == configured_connection,
                'Reloading an SSH alias replaced inherited values with overrides')
        provider = app.command('save_provider', provider={
            'id': '', 'name': 'SSH acceptance', 'protocol': 'openai-completions', 'baseUrl': endpoint + '/v1',
            'models': [{'id': 'gpt-4.1', 'name': 'Fixture', 'contextWindow': 128000, 'maxOutputTokens': 4096}],
        }, apiKey=FIXTURE_KEY)
        require_provider_key(app, provider['id'], FIXTURE_KEY)
        require_local_secret(app_directory, provider['id'], FIXTURE_KEY)
        configured_session = app.command('create_session', projectId=configured_project['id'],
                                          providerId=provider['id'], modelId='gpt-4.1')
        configured_events = app.turn(configured_session,
                                     'fixture:chat Verify the configured SSH alias through the SDK transport.')
        require_context_usage(configured_events, 128000)
        require(any(event['kind'] == 'message' and event['data'].get('content') == 'TANDEM_CHAT_OK'
                    for event in configured_events),
                'The configured SSH alias did not complete an SDK work turn')
        app.wait_title(configured_session['id'], configured_events)
        app.command('delete_session', sessionId=configured_session['id'])
        app.command('delete_project', id=configured_project['id'])
        app.command('save_preferences', telemetry={'enabled': True, 'endpoint': endpoint,
                    'protocol': 'http/json', 'captureContent': False}, cliPath=None)
        project = app.command('create_project', name='SSH work', path=str(workspace), connection=ssh.connection)
        fleet_session, _ = check_fleet(app, fixture, project, provider, 'gpt-4.1', workspace)
        app.command('delete_session', sessionId=fleet_session['id'])
        grouped_session, _ = check_grouped_activity(app, project, provider, 'gpt-4.1', workspace)
        app.command('delete_session', sessionId=grouped_session['id'])
        for session, _inputs, _marker, _metadata in check_attachments(app, fixture, project, provider, 'gpt-4.1'):
            app.command('delete_session', sessionId=session['id'])
        skill_session, _ = check_project_skills(app, fixture, project, provider, 'gpt-4.1', workspace, ssh.home)
        app.command('delete_session', sessionId=skill_session['id'])
        work = app.command('create_session', projectId=project['id'], providerId=provider['id'],
                           modelId='gpt-4.1', permissionMode='ask')
        first_work_events = app.turn(work, 'fixture:work-create Create verified.txt on the SSH host.')
        require(app.wait_title(work['id'], first_work_events) == 'Acceptance file operations', 'SSH work title was not summarized')
        require((workspace / 'verified.txt').read_text() == 'first version\n', 'Remote SDK tool did not create the file')
        commands_before_skills = ssh.command_count()
        skill_catalog = app.command('list_project_skills', projectId=project['id'])
        require(app.command('list_project_skills', projectId=project['id']) == skill_catalog,
                'Repeated skill discovery changed an unchanged project catalog')
        require(ssh.command_count() == commands_before_skills,
                'Skill discovery started a new SSH connection instead of reusing idle work')
        failed_tool = app.turn(work, 'fixture:work-create Attempt to create the already existing file.')
        require(any(tool['status'] == 'error' for tool in activity_tools(failed_tool)),
                'Failed remote tool was presented as a successful completion')
        require((workspace / 'verified.txt').read_text() == 'first version\n', 'Failed create changed the existing file')
        second = app.command('create_session', projectId=project['id'], providerId=provider['id'], modelId='gpt-4.1')
        check_independent_stop(app, work, second, ssh, fixture)
        app.command('send_message', sessionId=work['id'], content='fixture:permission Create the sibling file after approval.',
                    messageId=str(uuid.uuid4()))
        permission = app.wait_event(work['id'], lambda event: event['kind'] == 'permission')
        pending = app.command('snapshot')['activeTurns'][work['id']]['permissions']
        require(any(item['requestId'] == permission['data']['requestId'] for item in pending),
                'Pending permission could not be restored after a UI reconnect')
        app.command('respond_permission', sessionId=work['id'], requestId=permission['data']['requestId'], approved=True)
        app.wait_event(work['id'], lambda event: event['kind'] == 'status' and event['data']['status'] == 'idle')
        require((workspace.parent / 'approved.txt').is_file(), 'Restored permission did not authorize the actual tool')
        require(not app.command('snapshot')['activeTurns'].get(work['id'], {}).get('permissions'),
                'Resolved approval remained in the reconnect snapshot')
        check_remote_runtime_restart(app, work, ssh, fixture)
        children = child_processes(app.process.pid)
        ssh_processes = []
        for process in children:
            command = subprocess.run(['ps', '-p', str(process), '-o', 'comm='], capture_output=True, text=True, check=False).stdout.strip()
            if Path(command).name == 'ssh':
                ssh_processes.append(process)
            arguments = subprocess.run(['ps', '-p', str(process), '-o', 'command='], capture_output=True, text=True, check=False).stdout
            require(FIXTURE_KEY not in arguments, 'Credential appeared in child-process arguments')
        require(len(ssh_processes) == 1, 'Expected one persistent SSH transport for one remote work session')
        os.kill(ssh_processes[0], signal.SIGTERM)
        app.wait_event(work['id'], lambda event: event['kind'] == 'error')
        app.turn(work, 'fixture:work-edit Resume after reconnect and edit verified.txt.')
        require((workspace / 'verified.txt').read_text() == 'verified by SDK\n', 'Remote reconnect did not resume executable work')
        check_permission_modes(app, project, provider, 'gpt-4.1', workspace)
        check_ssh_reasoning(app, fixture, app_directory, endpoint, project)
        app.command('save_preferences', telemetry={'enabled': False, 'endpoint': '',
                    'protocol': 'http/json', 'captureContent': False}, cliPath=None)
        require(any(path == '/v1/traces' and 'json' in content_type and json_span_names(body)
                    for path, body, content_type in fixture.exports[exports_before:]),
                'SSH-hosted SDK did not export valid OTLP spans')
        require(ssh.known_hosts.read_bytes() == trusted_before, 'Production transport modified the trusted-host fixture')
        require_no_persisted_key(ssh.home)
        leaks = scan_for_key(app_directory, FIXTURE_KEY, allow_local_vault=True)
        require(not leaks, f'Local SSH host leaked credentials outside its credential table: {leaks}')
    finally:
        if app:
            app.close()
        ssh.close()


def stage_binaries(remote_server, dev_server, directory):
    """Snapshot both roles with sibling names used by production artifact lookup."""
    remote_snapshot = directory / debug_binary('tandem-server').name
    shutil.copy2(remote_server, remote_snapshot)
    dev_snapshot = None
    if dev_server is not None:
        dev_snapshot = directory / debug_binary('tandem-dev-server').name
        shutil.copy2(dev_server, dev_snapshot)
    return remote_snapshot, dev_snapshot


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, default=debug_binary('tandem-server'),
                        help='Remote agent executable deployed over SSH (tandem-server)')
    parser.add_argument('--dev-server', type=Path, default=debug_binary('tandem-dev-server'),
                        help='Local HTTP development host executable (tandem-dev-server)')
    parser.add_argument('--skip-ssh', action='store_true',
                        help='Verify stdio and supported local proxies without an SSH daemon')
    arguments = parser.parse_args()
    remote_server = arguments.server.resolve()
    dev_server = None if arguments.skip_ssh else arguments.dev_server.resolve()
    require(remote_server.is_file(), 'Build tandem-server first, or supply its remote executable with --server')
    if dev_server is not None:
        require(dev_server.is_file(), 'Build tandem-dev-server first, or supply the HTTP host with --dev-server')
    with tempfile.TemporaryDirectory(prefix='tandem-remote-e2e-') as temporary:
        root = Path(temporary)
        # Parallel desktop development may replace target/debug while tests run.
        remote_server, dev_server = stage_binaries(remote_server, dev_server, root)
        workspace = root / 'fixture-project'
        workspace.mkdir()
        fixture = Fixture(workspace)
        endpoint = ThreadingHTTPServer(('127.0.0.1', 0), handler_for(fixture))
        threading.Thread(target=endpoint.serve_forever, daemon=True).start()
        endpoint_url = f'http://127.0.0.1:{endpoint.server_port}'
        checks = []
        try:
            check_stdio(remote_server, root / 'stdio-home', endpoint_url, fixture)
            checks.append('stdio: real SDK streaming, Fleet worker lifecycle and isolated output/tools with ordinary tools restored after opt-out, native Anthropic thinking deltas and completion with opaque signatures excluded, image/text attachments on all protocols and PDF on Responses/Anthropic, attachment-only preparation and direct sends in Chat and Work, project skill discovery and invocation, same-session cold resume with a new provider/model and original history, reasoning changes and reset on the provider wire, EOF cleanup, malformed/oversized input, OTLP flush')
            if os.name == 'posix':
                check_daemon(remote_server, endpoint_url, fixture)
                checks.append('Unix daemon: five simultaneous proxy connections share one daemon and Copilot CLI; concurrent turns, session and approval isolation, prompt abort, proxy EOF/kill isolation, same-CLI reconnect with preserved history, shared CLI failure propagation and explicit recovery, and no interrupted-turn replay')
            if not arguments.skip_ssh:
                require(os.name == 'posix', 'Use --skip-ssh on hosts without a POSIX SSH daemon')
                check_ssh(remote_server, dev_server, root, endpoint_url, fixture)
                checks.append('OpenSSH: strict host trust, key rejection, concurrent atomic deployments share one daemon and CLI, literal directories, Fleet opt-in and isolated worker traces/permissions/cancellation, image/text attachments, grouped tool activity, confirmed skill selection metadata, repeated skill discovery reuses idle work without new SSH connections, remote tools, reconnect/resume, native thinking history and cancellation, core session reasoning changes, pending approval, and native-default reset with complete history')
            print(json.dumps({'result': 'passed', 'checks': checks, 'inferenceRequests': len(fixture.requests),
                              'telemetryExports': len(fixture.exports)}, indent=2))
        finally:
            endpoint.shutdown()
            endpoint.server_close()


if __name__ == '__main__':
    main()
