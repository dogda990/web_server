-- One 10 MiB POST per request; the endpoint consumes it and returns a short body.
local payload = string.rep("x", 10 * 1024 * 1024)

request = function()
  wrk.method = "POST"
  wrk.headers["Content-Type"] = "application/octet-stream"
  wrk.body = payload
  return wrk.format(nil, "/rps_heavy?delay_ms=0")
end

done = function(summary, latency, requests)
  io.write(string.format("CUSTOM_P95_MS: %.3f\n", latency:percentile(95.0) / 1000))
  io.write(string.format("CUSTOM_P99_MS: %.3f\n", latency:percentile(99.0) / 1000))
end
