"""Local protocol regression checks for the release server binary."""

import concurrent.futures
import http.client
import os
import socket
import subprocess
import time
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target/release/kubstu_web_server'


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def request(port, method='GET', path='/', body=None):
    connection = http.client.HTTPConnection('127.0.0.1', port, timeout=5)
    try:
        connection.request(method, path, body=body)
        response = connection.getresponse()
        return response.status, response.read()
    finally:
        connection.close()


def run_server(interface, app, checks):
    port = free_port()
    env = os.environ.copy()
    env['VIRTUAL_ENV'] = str(ROOT / '.venv')
    process = subprocess.Popen(
        [str(BINARY), '--interface', interface, '--working-dir', str(ROOT / 'tests'),
         '--host', '127.0.0.1', '--port', str(port), '--workers', '1',
         '--runtime-threads', '2', '--python-threads', '1', '--python-queue-capacity', '1',
         '--max-body-bytes', '16', app],
        cwd=ROOT, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
    )
    try:
        for _ in range(100):
            if process.poll() is not None:
                raise AssertionError(process.stderr.read().decode())
            try:
                with socket.create_connection(('127.0.0.1', port), timeout=0.1):
                    break
            except OSError:
                time.sleep(0.02)
        else:
            raise AssertionError('server did not start')
        checks(port)
    finally:
        process.terminate()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=3)


def check_wsgi(port):
    assert request(port) == (200, b'ab')
    # The server rejects Content-Length before collecting the oversized body.
    with socket.create_connection(('127.0.0.1', port), timeout=5) as sock:
        sock.sendall(b'POST / HTTP/1.1\r\nHost: localhost\r\nContent-Length: 17\r\n\r\n')
        assert b'413 Payload Too Large' in sock.recv(512)
    with concurrent.futures.ThreadPoolExecutor(max_workers=12) as pool:
        futures = [pool.submit(request, port, 'GET', '/slow') for _ in range(12)]
        statuses = [future.result()[0] for future in futures]
    assert 503 in statuses, statuses
    assert 200 in statuses, statuses


def check_asgi(port):
    assert request(port, 'POST', '/', b'hello') == (200, b'hello!')
    assert request(port, 'GET', '/') == (200, b'!')


if __name__ == '__main__':
    run_server('wsgi', 'fixture_apps:wsgi_app', check_wsgi)
    run_server('asgi', 'fixture_apps:asgi_app', check_asgi)
    print('WSGI write(), HTTP 413/503, and ASGI request/response checks passed')
