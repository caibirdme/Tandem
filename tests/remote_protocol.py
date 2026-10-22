"""Bounded process and frame helpers for real remote protocol acceptance."""

from __future__ import annotations

import json
import os
import queue
import shutil
import subprocess
import threading
import time
import uuid

from inference_fixture import require


def private_environment(home):
    environment = os.environ.copy()
    environment.update(HOME=str(home), USERPROFILE=str(home), XDG_CACHE_HOME=str(home / '.cache'),
                       XDG_CONFIG_HOME=str(home / '.config'), XDG_STATE_HOME=str(home / '.state'))
    # Acceptance assertions require complete traces, not the host's sampling policy.
    for key in ('COPILOT_CLI_PATH', 'GITHUB_TOKEN', 'GH_TOKEN', 'COPILOT_GITHUB_TOKEN',
                'OTEL_TRACES_SAMPLER', 'OTEL_TRACES_SAMPLER_ARG'):
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


class StdioServer:
    def __init__(self, binary, directory, mode='--stdio', arguments=None, environment=None):
        self.owns_runtime = mode == '--stdio'
        self.closed = False
        self.log = open(directory / f'stdio-{uuid.uuid4()}.log', 'w+')
        self.process = subprocess.Popen(arguments or [str(binary), mode], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=self.log, text=True,
                                        env=environment or private_environment(directory))
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
        finally:
            self.responses.put(None)

    def request(self, command, **payload):
        self.next_id += 1
        self.process.stdin.write(json.dumps({'id': self.next_id, 'command': command, 'payload': payload}) + '\n')
        self.process.stdin.flush()
        response = self.responses.get(timeout=45)
        if response is None:
            raise RuntimeError(f'Remote connection closed before {command} completed')
        require(response.get('id') == self.next_id, 'Remote response ID did not match the request')
        return response

    def command(self, command, **payload):
        response = self.request(command, **payload)
        if 'error' in response:
            raise RuntimeError(response['error']['message'])
        if command == 'start_agent' and self.owns_runtime:
            self.children.update(child_processes(self.process.pid))
        return response['value']

    def turn(self, session_id, prompt, selected_skills=(), attachments=(), fleet=False):
        while not self.events.empty():
            self.events.get_nowait()
        self.command('send_agent', sessionId=session_id, content=prompt,
                     selectedSkills=list(selected_skills), attachments=list(attachments), fleet=fleet)
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
        if self.closed:
            return
        self.closed = True
        children = self.children.union(child_processes(self.process.pid)) if self.owns_runtime else set()
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
        if expected_status is not None:
            require((status == 0) == (expected_status == 0), f'Unexpected remote exit status: {status}')
        require(self.failures.empty(), 'Remote stdout contained non-protocol output')
        require_processes_stopped(children)
