import time

from django.http import HttpResponse, HttpResponseNotAllowed, JsonResponse
from django.views.decorators.csrf import csrf_exempt


def _json_payload(request, endpoint_name: str):
    body = request.body.decode("utf-8", errors="replace")
    return JsonResponse(
        {
            "endpoint": endpoint_name,
            "method": request.method,
            "path": request.path,
            "query": request.META.get("QUERY_STRING", ""),
            "content_type": request.META.get("CONTENT_TYPE", ""),
            "body": body,
            "headers": {
                key: value
                for key, value in request.META.items()
                if key.startswith("HTTP_") or key in {"CONTENT_TYPE", "CONTENT_LENGTH"}
            },
        }
    )


@csrf_exempt
def get_endpoint(request):
    if request.method != "GET":
        return HttpResponseNotAllowed(["GET"])
    return _json_payload(request, "get")


@csrf_exempt
def post_endpoint(request):
    if request.method != "POST":
        return HttpResponseNotAllowed(["POST"])
    return _json_payload(request, "post")


@csrf_exempt
def put_endpoint(request):
    if request.method != "PUT":
        return HttpResponseNotAllowed(["PUT"])
    return _json_payload(request, "put")


@csrf_exempt
def patch_endpoint(request):
    if request.method != "PATCH":
        return HttpResponseNotAllowed(["PATCH"])
    return _json_payload(request, "patch")


@csrf_exempt
def delete_endpoint(request):
    if request.method != "DELETE":
        return HttpResponseNotAllowed(["DELETE"])
    return _json_payload(request, "delete")


@csrf_exempt
def head_endpoint(request):
    if request.method != "HEAD":
        return HttpResponseNotAllowed(["HEAD"])
    return _json_payload(request, "head")


@csrf_exempt
def options_endpoint(request):
    if request.method != "OPTIONS":
        return HttpResponseNotAllowed(["OPTIONS"])

    response = _json_payload(request, "options")
    response["Allow"] = "OPTIONS"
    return response


@csrf_exempt
def methods_echo(request):
    allowed_methods = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]
    if request.method not in allowed_methods:
        return HttpResponseNotAllowed(allowed_methods)

    response = _json_payload(request, "methods-echo")
    response["Allow"] = ", ".join(allowed_methods)
    return response


def rps_plain(request):
    if request.method != "GET":
        return HttpResponseNotAllowed(["GET"])
    return HttpResponse(b"ok", content_type="text/plain")


@csrf_exempt
def rps_heavy(request):
    """Benchmark view with an optional request body and a controlled delay."""
    if request.method not in {"GET", "POST"}:
        return HttpResponseNotAllowed(["GET", "POST"])

    if request.method == "POST":
        body_size = len(request.body)
    else:
        body_size = 0

    try:
        delay_ms = int(request.GET.get("delay_ms", "25"))
    except ValueError:
        return HttpResponse(b"delay_ms must be an integer", status=400)
    if delay_ms < 0 or delay_ms > 100:
        return HttpResponse(b"delay_ms must be between 0 and 100", status=400)

    time.sleep(delay_ms / 1000)
    response = HttpResponse(b"ok", content_type="text/plain")
    response["X-Request-Body-Bytes"] = str(body_size)
    return response
