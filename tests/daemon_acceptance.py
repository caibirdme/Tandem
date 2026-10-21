"""Verify shared Unix runtime ownership using production proxies and the real SDK."""

from __future__ import annotations

import os
import queue
import shutil
import signal
import subprocess
import tempfile
import threading
import time
import uuid
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

from credential_checks import scan_for_key
from inference_fixture import FIXTURE_KEY, require
from model_switch_acceptance import require_wire_history
from remote_protocol import StdioServer, child_processes, require_processes_stopped


def process_command(process):
    return subprocess.run(
        ['ps', '-p', str(process), '-o', 'comm='],
        capture_output=True, text=True, check=False,
    ).stdout.strip()


def runtime_processes(daemon):
    return [process for process in child_processes(daemon)
            if Path(process_command(process)).name in {'copilot', 'copilot-runtime'}]


def require_runtime(daemon, expected=None):
    runtimes = runtime_processes(daemon)
    require(len(runtimes) == 1, 'Independent proxies did not share exactly one Copilot CLI')
    if expected is not None:
        require(runtimes[0] == expected, 'A proxy lifecycle change restarted the shared Copilot CLI')
    return runtimes[0]


def stop_daemon(daemon):
    children = child_processes(daemon)
    try:
        os.kill(daemon, signal.SIGTERM)
    except ProcessLookupError:
        pass
    require_processes_stopped(children)
    require_processes_stopped([daemon])


def wait_event(client, session_id, predicate, timeout=30):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            event = client.events.get(timeout=max(.01, deadline - time.monotonic()))
        except queue.Empty:
            break
        require(event['sessionId'] == session_id, 'Daemon leaked an event into another proxy connection')
        if predicate(event):
            return event
        require(event['kind'] != 'error', 'Unexpected remote session failure')
    raise TimeoutError('Expected shared-runtime event did not arrive')


def require_error(client, command, codes, **payload):
    response = client.request(command, **payload)
    require(response.get('error', {}).get('code') in codes,
            f'{command} did not reject cross-connection or expired session state')


def options_for(session_id, endpoint, directory=None):
    return {
        'sessionId': session_id,
        'mode': {'kind': 'work', 'directory': str(directory)} if directory else {'kind': 'chat'},
        'permissionMode': 'ask',
        'provider': {'protocol': 'openai-completions', 'baseUrl': endpoint + '/v1', 'apiKey': FIXTURE_KEY},
        'model': {'id': 'gpt-4.1', 'contextWindow': 128000, 'maxOutputTokens': 4096},
        'systemPrompt': 'Use the requested fixture response.',
    }


def check_daemon(binary, endpoint, fixture):
    with tempfile.TemporaryDirectory(prefix='td-') as temporary:
        home = Path(temporary)
        installed = home / 'tandem-server'
        shutil.copy2(binary, installed)
        binary = installed
        workspace = home / 'project'
        workspace.mkdir()
        previous_workspace = fixture.workspace
        fixture.workspace = workspace
        clients = []
        daemons = set()
        lock = threading.Lock()
        telemetry = {'enabled': True, 'endpoint': endpoint, 'protocol': 'http/json', 'captureContent': False}
        options = [options_for(str(uuid.uuid4()), endpoint, workspace) for _ in range(5)]
        second_workspace = home / 'second-project'
        second_workspace.mkdir()
        options[4]['mode'] = {'kind': 'work', 'directory': str(second_workspace)}
        options[4]['provider'].update(protocol='anthropic', baseUrl=endpoint)
        options[4]['model']['id'] = 'claude-sonnet-4.5'
        barrier = threading.Barrier(len(options))

        def connect(option):
            barrier.wait(timeout=10)
            client = StdioServer(binary, home, mode='--proxy')
            with lock:
                clients.append(client)
            info = client.command('server_info')
            require(type(info.get('processId')) is int, 'Remote diagnostics did not identify the shared daemon')
            with lock:
                daemons.add(info['processId'])
            client.command('start_agent', options=option, resume=False, telemetry=telemetry)
            return client

        try:
            with ThreadPoolExecutor(max_workers=len(options)) as executor:
                connected = list(executor.map(connect, options))
            require(len(daemons) == 1, 'Concurrent proxy startup created more than one remote daemon')
            daemon = next(iter(daemons))
            runtime = require_runtime(daemon)
            for client in connected:
                owned = {process for process in child_processes(client.process.pid)
                         if Path(process_command(process)).name in {'copilot', 'copilot-runtime'}}
                require(owned <= {runtime}, 'A proxy launched an additional Copilot CLI')
            for process in [daemon, *child_processes(daemon), *(client.process.pid for client in connected)]:
                arguments = subprocess.run(
                    ['ps', '-p', str(process), '-o', 'command='],
                    capture_output=True, text=True, check=False,
                ).stdout
                require(FIXTURE_KEY not in arguments, 'Provider credential appeared in daemon or proxy arguments')

            catalog = StdioServer(binary, home, mode='--proxy')
            clients.append(catalog)
            replacement = home / 'tandem-server-next'
            shutil.copy2(binary, replacement)
            replacement.replace(binary)
            reinstalled = StdioServer(binary, home, mode='--proxy')
            clients.append(reinstalled)
            reinstalled_daemon = reinstalled.command('server_info')['processId']
            daemons.add(reinstalled_daemon)
            require(reinstalled_daemon == daemon, 'An identical atomic binary replacement created another daemon')
            reinstalled.close()
            require_runtime(daemon, runtime)
            require(catalog.command('list_project_skills', directory=str(workspace), telemetry=telemetry) == [],
                    'An empty remote project produced unexpected skill metadata')
            require_error(catalog, 'list_project_skills', {'runtime_busy'}, directory=str(workspace),
                          telemetry={'enabled': False})
            catalog.close()
            require_runtime(daemon, runtime)

            first_prompt = 'fixture:model-switch Keep SHARED_DAEMON_ORIGINAL_HISTORY across a proxy reconnect.'
            first_response = 'TANDEM_MODEL_SWITCH_OK gpt-4.1'
            connected[0].turn(options[0]['sessionId'], first_prompt)
            fixture.release_slow_response.clear()
            connected[1].command('send_agent', sessionId=options[1]['sessionId'],
                                 content='fixture:slow Hold one remote conversation while others finish.')
            wait_event(connected[1], options[1]['sessionId'], lambda event: event['kind'] == 'delta')

            def finish_independent(index):
                return connected[index].turn(options[index]['sessionId'],
                                             f'fixture:chat Finish concurrent remote conversation {index}.')

            with ThreadPoolExecutor(max_workers=3) as executor:
                turns = list(executor.map(finish_independent, range(2, 5)))
            require(all(any(event['kind'] == 'message' for event in events) for events in turns),
                    'A streaming session blocked unrelated remote conversations')
            independent = [(path, body) for path, body in fixture.conversation_requests()
                           if 'Finish concurrent remote conversation 4.' in str(body)]
            require(len(independent) == 1 and independent[0][0].split('?', 1)[0] == '/v1/messages'
                    and independent[0][1]['model'] == 'claude-sonnet-4.5'
                    and str(second_workspace) in str(independent[0][1]),
                    'Shared runtime mixed session provider, model, or working directory configuration')
            started = time.monotonic()
            connected[1].command('abort_agent', sessionId=options[1]['sessionId'])
            require(time.monotonic() - started < 5, 'Remote control waited behind a long-running operation')
            fixture.release_slow_response.set()
            wait_event(connected[1], options[1]['sessionId'],
                       lambda event: event['kind'] == 'status' and event['data']['state'] == 'idle')
            require_runtime(daemon, runtime)

            require_error(connected[2], 'start_agent', {'runtime_busy', 'session_not_connected'},
                          options=options[0], resume=True, telemetry=telemetry)
            for command in ('abort_agent', 'disconnect_agent'):
                require_error(connected[2], command, {'runtime_busy', 'session_not_connected'},
                              sessionId=options[0]['sessionId'])
            prepared_id = str(uuid.uuid4())
            connected[2].command('prepare_agent_message', sessionId=options[2]['sessionId'],
                                 preparedId=prepared_id, content='fixture:chat Reserve one explicit prepared message.')
            require_error(connected[4], 'send_prepared_agent_message', {'invalid_payload'},
                          sessionId=options[4]['sessionId'], preparedId=prepared_id)
            connected[2].command('abort_agent', sessionId=options[2]['sessionId'])
            require_error(connected[2], 'send_prepared_agent_message', {'invalid_payload'},
                          sessionId=options[2]['sessionId'], preparedId=prepared_id)
            connected[0].command('send_agent', sessionId=options[0]['sessionId'],
                                 content='fixture:cancel-permission Leave this write awaiting approval.')
            permission = wait_event(connected[0], options[0]['sessionId'],
                                    lambda event: event['kind'] == 'permission')
            require_error(connected[2], 'approve_agent', {'runtime_busy', 'session_not_connected'},
                          sessionId=options[0]['sessionId'], requestId=permission['data']['requestId'], approved=True)
            require(not (home / 'cancelled.txt').exists(), 'Another proxy approved an unrelated pending write')
            fixture.release_slow_response.clear()
            connected[1].command('send_agent', sessionId=options[1]['sessionId'],
                                 content='fixture:slow Keep this active turn alive through another proxy crash.')
            wait_event(connected[1], options[1]['sessionId'], lambda event: event['kind'] == 'delta')
            connected[0].process.kill()
            connected[0].close(expected_status=None)
            fixture.release_slow_response.set()
            unaffected = connected[1].receive_turn(options[1]['sessionId'])
            require(any(event['kind'] == 'message' and event['data'].get('content') == 'TANDEM_SLOW_OK'
                        for event in unaffected), 'One proxy crash interrupted another active conversation')
            connected[2].turn(options[2]['sessionId'], 'fixture:chat Continue while another proxy disconnects.')
            require_runtime(daemon, runtime)

            resumed = StdioServer(binary, home, mode='--proxy')
            clients.append(resumed)
            require(resumed.command('server_info')['processId'] == daemon,
                    'Reconnecting a proxy replaced the remote daemon')
            deadline = time.monotonic() + 20
            while True:
                response = resumed.request('start_agent', options=options[0], resume=True, telemetry=telemetry)
                if 'error' not in response:
                    break
                require(response['error']['code'] == 'runtime_busy' and time.monotonic() < deadline,
                        'The lost connection did not release its session ownership')
                time.sleep(.05)
            require_error(resumed, 'approve_agent', {'approval_not_found'}, sessionId=options[0]['sessionId'],
                          requestId=permission['data']['requestId'], approved=True)
            offset = len(fixture.conversation_requests())
            prompt = 'fixture:model-switch Resume only this explicit new message with the original history.'
            resumed.turn(options[0]['sessionId'], prompt)
            requests = fixture.conversation_requests()[offset:]
            require(len(requests) == 1, 'Reconnect replayed an interrupted remote turn')
            require_wire_history(requests[0][1], [
                {'role': 'user', 'content': first_prompt},
                {'role': 'assistant', 'content': first_response},
            ])
            require(not (home / 'cancelled.txt').exists(), 'Disconnect or reconnect approved the pending write')
            require_runtime(daemon, runtime)

            # EOF releases only that proxy's sessions; a surviving client stays usable.
            connected[3].close()
            connected[4].turn(options[4]['sessionId'], 'fixture:chat Continue after another client closes cleanly.')
            require_runtime(daemon, runtime)

            # A CLI crash is shared by every live session; recovery requires a new explicit turn.
            fixture.release_slow_response.clear()
            for index in (1, 2):
                connected[index].command('send_agent', sessionId=options[index]['sessionId'],
                                         content='fixture:slow Preserve the interrupted turn when the CLI exits.')
                wait_event(connected[index], options[index]['sessionId'], lambda event: event['kind'] == 'delta')
            os.kill(runtime, signal.SIGKILL)
            fixture.release_slow_response.set()
            for index in (1, 2):
                wait_event(connected[index], options[index]['sessionId'], lambda event: event['kind'] == 'error')
            for client in clients:
                client.close(expected_status=None)
            recovered = StdioServer(binary, home, mode='--proxy')
            clients.append(recovered)
            recovered_daemon = recovered.command('server_info')['processId']
            daemons.add(recovered_daemon)
            offset = len(fixture.conversation_requests())
            recovered.command('start_agent', options=options[0], resume=True, telemetry=telemetry)
            recovered.turn(options[0]['sessionId'],
                           'fixture:model-switch Recover on this explicit message after the shared CLI exits.')
            requests = fixture.conversation_requests()[offset:]
            require(len(requests) == 1, 'CLI recovery replayed an interrupted turn')
            require_wire_history(requests[0][1], [
                {'role': 'user', 'content': first_prompt},
                {'role': 'assistant', 'content': first_response},
            ])
            require(require_runtime(recovered_daemon) != runtime, 'The failed shared CLI was not replaced')
        finally:
            fixture.release_slow_response.set()
            fixture.workspace = previous_workspace
            cleanup_errors = []
            for client in clients:
                try:
                    client.close(expected_status=None)
                except Exception as error:
                    cleanup_errors.append(error)
            for daemon in daemons:
                try:
                    stop_daemon(daemon)
                except Exception as error:
                    cleanup_errors.append(error)
            require(not scan_for_key(home, FIXTURE_KEY), 'Remote daemon persisted a provider credential')
            if cleanup_errors:
                raise RuntimeError('Shared remote runtime cleanup failed') from cleanup_errors[0]
