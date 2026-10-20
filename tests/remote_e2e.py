#!/usr/bin/env python3
"""Exercise real remote stdio and OpenSSH without touching the user's SSH setup.

The SDK and tools are production code; inference and OTLP use the same local
fixtures as runtime_e2e.py. SSH uses ephemeral keys, an isolated known-hosts file,
and an unprivileged localhost sshd. A forced command confines the test server's
home and runtime state to a temporary directory. No real provider key is needed.

Run after building tandem-server and tandem-dev-server: python3 tests/remote_e2e.py
Use --server for the remote stdio executable and --dev-server for the HTTP host.
With --skip-ssh, only the remote stdio executable is required.
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

from acceptance_support import Application, debug_binary, require_context_usage
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
    require_wire_effort,
    set_effort,
    wire_effort,
)
from skill_acceptance import check_project_skills, check_stdio_skills


def private_environment(home):
    environment = os.environ.copy()
    environment.update(HOME=str(home), XDG_CACHE_HOME=str(home / '.cache'),
                       XDG_CONFIG_HOME=str(home / '.config'), XDG_STATE_HOME=str(home / '.state'))
    for key in ('COPILOT_CLI_PATH', 'GITHUB_TOKEN', 'GH_TOKEN', 'COPILOT_GITHUB_TOKEN'):
        environment.pop(key, None)
    return environment


def child_processes(parent):
    """Inspect only descendants of a process owned by this test."""
    if not shutil.which('pgrep'):
        return []
    result = subprocess.run(['pgrep', '-P', str(parent)], capture_output=True, text=True, check=False)
    children = [int(value) for value in result.stdout.split()]
    return children + [descendant for child in children for descendant in child_processes(child)]


def require_processes_stopped(processes):
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        remaining = []
        for process in processes:
            try:
                os.kill(process, 0)
                remaining.append(process)
            except ProcessLookupError:
                pass
        if not remaining:
            return
        time.sleep(.05)
    raise AssertionError(f'Test-owned runtime children survived cleanup: {remaining}')


def require_no_persisted_key(home):
    leaks = scan_for_key(home, FIXTURE_KEY)
    require(not leaks, f'Provider credential persisted on remote host: {leaks}')


class StdioServer:
    def __init__(self, binary, directory):
        self.log = open(directory / f'stdio-{uuid.uuid4()}.log', 'w+')
        self.process = subprocess.Popen([str(binary), '--stdio'], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=self.log, text=True,
                                        env=private_environment(directory))
        self.responses = queue.Queue()
        self.events = queue.Queue()
        self.failures = queue.Queue()
        self.next_id = 0
        self.children = set()
        self.reader = threading.Thread(target=self.read_frames, daemon=True)
        self.reader.start()

    def read_frames(self):
        try:
            for line in self.process.stdout:
                frame = json.loads(line)
                if 'event' in frame:
                    self.events.put(frame['event'])
                else:
                    self.responses.put(frame)
        except (OSError, ValueError) as error:
            self.failures.put(error)

    def command(self, command, **payload):
        self.next_id += 1
        self.process.stdin.write(json.dumps({'id': self.next_id, 'command': command, 'payload': payload}) + '\n')
        self.process.stdin.flush()
        response = self.responses.get(timeout=45)
        require(response.get('id') == self.next_id, 'Remote response ID did not match the request')
        if 'error' in response:
            raise RuntimeError(response['error']['message'])
        if command == 'start_agent':
            self.children.update(child_processes(self.process.pid))
        return response['value']

    def turn(self, session_id, prompt, selected_skills=()):
        while not self.events.empty():
            self.events.get_nowait()
        self.command('send_agent', sessionId=session_id, content=prompt,
                     selectedSkills=list(selected_skills))
        return self.receive_turn(session_id)

    def prepared_turn(self, session_id, prepared_id):
        while not self.events.empty():
            self.events.get_nowait()
        self.command('send_prepared_agent_message', sessionId=session_id, preparedId=prepared_id)
        return self.receive_turn(session_id)

    def receive_turn(self, session_id):
        received = []
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            event = self.events.get(timeout=max(1, deadline - time.monotonic()))
            require(event['sessionId'] == session_id, 'Remote event routed to another session')
            received.append(event)
            if event['kind'] == 'permission':
                self.command('approve_agent', sessionId=session_id,
                             requestId=event['data']['requestId'], approved=True)
            if event['kind'] == 'error':
                raise AssertionError(event['data'])
            if event['kind'] == 'status' and event['data']['state'] == 'idle':
                return received
        raise TimeoutError('Remote stdio turn did not finish')

    def close(self, expected_status=0):
        children = self.children.union(child_processes(self.process.pid))
        if self.process.stdin and not self.process.stdin.closed:
            try:
                self.process.stdin.close()
            except BrokenPipeError:
                pass
        try:
            status = self.process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
            raise AssertionError('Remote server did not exit after stdin EOF') from None
        finally:
            self.reader.join(timeout=2)
            self.process.stdout.close()
            self.log.close()
        require((status == 0) == (expected_status == 0), f'Unexpected remote exit status: {status}')
        require(self.failures.empty(), 'Remote stdout contained non-protocol output')
        require_processes_stopped(children)


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
        check_stdio_skills(server, fixture, options)
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
    for malformed in ('{invalid-json}\n', 'x' * (8 * 1024 * 1024 + 1) + '\n'):
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
    for index, effort in enumerate((None, 'low', 'high', None)):
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


class IsolatedSsh:
    def __init__(self, directory):
        self.directory = directory
        directory.mkdir()
        self.home = directory / 'remote-home'
        self.home.mkdir()
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
        forced.write_text('import os, time\nfrom pathlib import Path\n'
                          f'os.environ.update({remote_environment!r})\n'
                          f'os.chdir({str(self.home)!r})\n'
                          'delay = Path("delay-bootstrap")\n'
                          'if "uname -s" in os.environ["SSH_ORIGINAL_COMMAND"] and delay.exists():\n'
                          '    Path("bootstrap-waiting").touch()\n'
                          '    while delay.exists(): time.sleep(.05)\n'
                          'os.execv("/bin/sh", ["sh", "-c", os.environ["SSH_ORIGINAL_COMMAND"]])\n')
        config = directory / 'sshd_config'
        config.write_text('\n'.join([
            f'Port {self.port}', 'ListenAddress 127.0.0.1', f'HostKey {directory}/host',
            f'PidFile {directory}/sshd.pid', f'AuthorizedKeysFile {directory}/identity.pub',
            'PasswordAuthentication no', 'KbdInteractiveAuthentication no', 'UsePAM no',
            'StrictModes no', 'PermitUserRC no', 'AllowTcpForwarding no', 'AllowAgentForwarding no',
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
        wrapper = wrapper_directory / 'ssh'
        wrapper.write_text('#!/bin/sh\nexec ' + shlex.join([
            ssh, '-F', '/dev/null', '-o', f'UserKnownHostsFile={self.known_hosts}',
            '-o', 'GlobalKnownHostsFile=/dev/null', '-o', 'IdentitiesOnly=yes',
        ]) + ' "$@"\n')
        wrapper.chmod(0o700)
        self.environment = os.environ.copy()
        self.environment['PATH'] = str(wrapper_directory) + os.pathsep + self.environment.get('PATH', '')
        self.connection = {'kind': 'ssh', 'host': '127.0.0.1', 'port': self.port,
                           'user': getpass.getuser(), 'identityFile': str(directory / 'identity')}

    def trust_fixture_key(self):
        key_type, key, *_ = (self.directory / 'host.pub').read_text().split()
        self.known_hosts.write_text(f'[127.0.0.1]:{self.port} {key_type} {key}\n')

    def close(self):
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


def check_independent_stop(app, work, second, ssh, fixture):
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
    messages = app.command('messages', sessionId=work['id'])
    require(any(message.get('interrupted') and message['content'] for message in messages),
            'Stopped streamed output was not persisted as an interrupted message')
    app.command('delete_session', sessionId=second['id'])


def check_remote_runtime_restart(app, work, ssh, fixture):
    fixture.release_slow_response.clear()
    app.command('send_message', sessionId=work['id'], content='fixture:slow Wait while the remote runtime exits.',
                messageId=str(uuid.uuid4()))
    app.wait_event(work['id'], lambda event: event['kind'] == 'delta')
    runtimes = []
    for process in child_processes(ssh.daemon.pid):
        command = subprocess.run(['ps', '-p', str(process), '-o', 'comm='],
                                 capture_output=True, text=True, check=False).stdout.strip()
        if Path(command).name in {'copilot', 'copilot-runtime'}:
            runtimes.append(process)
    require(len(runtimes) == 1, 'Could not identify this test SSH daemon\'s sole SDK child')
    os.kill(runtimes[0], signal.SIGKILL)
    fixture.release_slow_response.set()
    app.wait_event(work['id'], lambda event: event['kind'] == 'error')
    requests_before = len(fixture.conversation_requests())
    app.turn(work, 'fixture:chat Recover the remote runtime on this explicit new message.')
    require(len(fixture.conversation_requests()) == requests_before + 1, 'Remote runtime recovery replayed the interrupted prompt')


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
    for effort in ('high', 'low'):
        work = set_effort(app, directory, work, effort)
        check_reasoning_turn(app, fixture, work,
                             f'fixture:chat Apply SSH reasoning override {effort} in the original conversation.', effort)
    work = set_effort(app, directory, work, 'medium')
    work = check_pending_reasoning_change(app, fixture, directory, work)
    work = set_effort(app, directory, work, None)
    check_reasoning_turn(app, fixture, work,
                         'fixture:chat Reset SSH reasoning to the original native default.', native_effort)
    app.command('delete_session', sessionId=work['id'])


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
        expect_command_error(app, 'check_connection', 'host key', connection=ssh.connection)
        require(ssh.known_hosts.read_text() == '', 'Unknown SSH host was silently trusted')
        ssh.trust_fixture_key()
        trusted_before = ssh.known_hosts.read_bytes()
        wrong_key = {**ssh.connection, 'identityFile': str(ssh.directory / 'wrong-identity')}
        expect_command_error(app, 'check_connection', 'authentication', connection=wrong_key)
        app.command('check_connection', connection=ssh.connection)
        deployed = ssh.home / '.local/share/tandem/bin/tandem-server'
        require(deployed.is_file(), 'SSH bootstrap did not deploy tandem-server')
        require(deployed.stat().st_mode & 0o777 == 0o700, 'Deployed server is not private and executable')
        require(digest(deployed) == digest(remote_server), 'SSH deployed bytes differ from tandem-server (the remote artifact)')
        modification = deployed.stat().st_mtime_ns
        app.command('check_connection', connection=ssh.connection)
        require(deployed.stat().st_mtime_ns == modification, 'Matching server version was needlessly replaced')
        deployed.write_text('#!/bin/sh\n[ "$1" = "--version" ] || exit 97\nprintf "tandem-server 0.1.3\\n"\n')
        app.command('check_connection', connection=ssh.connection)
        require(digest(deployed) == digest(remote_server),
                'An older server was reused instead of receiving the updated runtime configuration')
        workspace = ssh.home / "project ' $(exit 91)"
        workspace.mkdir()
        fixture.workspace = workspace
        listing = app.command('browse_directory', connection=ssh.connection, path='~')
        require(any(entry['name'] == workspace.name for entry in listing['entries']), 'SSH directory browsing lost literal path')
        provider = app.command('save_provider', provider={
            'id': '', 'name': 'SSH acceptance', 'protocol': 'openai-completions', 'baseUrl': endpoint + '/v1',
            'models': [{'id': 'gpt-4.1', 'name': 'Fixture', 'contextWindow': 128000, 'maxOutputTokens': 4096}],
        }, apiKey=FIXTURE_KEY)
        require_provider_key(app, provider['id'], FIXTURE_KEY)
        require_local_secret(app_directory, provider['id'], FIXTURE_KEY)
        app.command('save_preferences', telemetry={'enabled': True, 'endpoint': endpoint,
                    'protocol': 'http/json', 'captureContent': False}, cliPath=None)
        project = app.command('create_project', name='SSH work', path=str(workspace), connection=ssh.connection)
        skill_session, _ = check_project_skills(app, fixture, project, provider, 'gpt-4.1', workspace)
        app.command('delete_session', sessionId=skill_session['id'])
        work = app.command('create_session', projectId=project['id'], providerId=provider['id'], modelId='gpt-4.1')
        first_work_events = app.turn(work, 'fixture:work-create Create verified.txt on the SSH host.')
        require(app.wait_title(work['id'], first_work_events) == 'Acceptance file operations', 'SSH work title was not summarized')
        require((workspace / 'verified.txt').read_text() == 'first version\n', 'Remote SDK tool did not create the file')
        failed_tool = app.turn(work, 'fixture:work-create Attempt to create the already existing file.')
        require(any(event['kind'] == 'tool' and event['data']['status'] == 'error' for event in failed_tool),
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
                        help='Remote stdio executable deployed over SSH (tandem-server)')
    parser.add_argument('--dev-server', type=Path, default=debug_binary('tandem-dev-server'),
                        help='Local HTTP development host executable (tandem-dev-server)')
    parser.add_argument('--skip-ssh', action='store_true', help='Only verify stdio on hosts without an SSH daemon')
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
            checks.append('stdio: real SDK streaming, project skill discovery and invocation, same-session cold resume with a new provider/model and original history, reasoning changes and reset on the provider wire, EOF cleanup, malformed/oversized input, OTLP flush')
            if not arguments.skip_ssh:
                require(os.name == 'posix', 'Use --skip-ssh on hosts without a POSIX SSH daemon')
                check_ssh(remote_server, dev_server, root, endpoint_url, fixture)
                checks.append('OpenSSH: strict host trust, key rejection, atomic deployment, literal directories, project skills, remote tools, reconnect/resume, core session reasoning changes, pending approval, and native-default reset with complete history')
            print(json.dumps({'result': 'passed', 'checks': checks, 'inferenceRequests': len(fixture.requests),
                              'telemetryExports': len(fixture.exports)}, indent=2))
        finally:
            endpoint.shutdown()
            endpoint.server_close()


if __name__ == '__main__':
    main()
