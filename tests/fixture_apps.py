import asyncio


def wsgi_app(environ, start_response):
    if environ['PATH_INFO'] == '/slow':
        import time
        time.sleep(0.25)
    write = start_response('200 OK', [('Content-Type', 'text/plain')])
    write(b'a')
    return [b'b']


async def asgi_app(scope, receive, send):
    event = await receive()
    assert event['type'] == 'http.request'
    await send({'type': 'http.response.start', 'status': 200, 'headers': [(b'content-type', b'text/plain')]})
    await send({'type': 'http.response.body', 'body': event.get('body', b''), 'more_body': True})
    await send({'type': 'http.response.body', 'body': b'!', 'more_body': False})
