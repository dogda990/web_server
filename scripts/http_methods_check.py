#!/usr/bin/env python3
import argparse
import http.client
import json


METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"]


def send_method(host: str, port: int, method: str):
    conn = http.client.HTTPConnection(host, port, timeout=10)
    path = "/" + method.lower()
    body = None
    headers = {}

    if method in {"POST", "PUT", "PATCH", "DELETE"}:
        body = f"{method}_PAYLOAD".encode("utf-8")
        headers["Content-Type"] = "text/plain"
    elif method == "OPTIONS":
        body = b'{"ok": true}'
        headers["Content-Type"] = "application/json"

    conn.request(method, path, body=body, headers=headers)
    response = conn.getresponse()
    payload = response.read()
    conn.close()

    if method == "HEAD":
        return {
            "method": method,
            "status": response.status,
            "reason": response.reason,
            "body_bytes": len(payload),
            "echoed_method": None,
            "raw_body": payload.decode("utf-8", errors="replace"),
        }

    echoed_method = None
    raw_body = payload.decode("utf-8", errors="replace")
    try:
        echoed_method = json.loads(raw_body).get("method")
    except Exception:
        echoed_method = "(invalid-json)"

    return {
        "method": method,
        "status": response.status,
        "reason": response.reason,
        "body_bytes": len(payload),
        "echoed_method": echoed_method,
        "raw_body": raw_body,
    }


def main():
    parser = argparse.ArgumentParser(description="Smoke test all HTTP methods against Django test endpoints")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()

    print(f"Smoke test http://{args.host}:{args.port}")
    for method in METHODS:
        result = send_method(args.host, args.port, method)
        if method == "HEAD":
            print(f"{method}: {result['status']} {result['reason']}, body_bytes={result['body_bytes']}")
        else:
            suffix = ""
            if result["echoed_method"] == "(invalid-json)":
                snippet = result["raw_body"].strip().replace("\n", " ")
                if len(snippet) > 140:
                    snippet = snippet[:140] + "..."
                suffix = f", body={snippet!r}"
            print(
                f"{method}: {result['status']} {result['reason']}, "
                f"echoed_method={result['echoed_method']}{suffix}"
            )


if __name__ == "__main__":
    main()
