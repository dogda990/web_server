-- Identical request for all five ablations and the integrated comparison.
-- The body is intentionally non-empty so the request-buffer variant is exercised.
request = function()
  wrk.method = "POST"
  wrk.headers["Content-Type"] = "application/octet-stream"
  wrk.body = string.rep("x", 1024)
  return wrk.format(nil, "/post")
end

done = function(summary, latency, requests)
  io.write(string.format("CUSTOM_P95_MS: %.3f\n", latency:percentile(95.0) / 1000))
  io.write(string.format("CUSTOM_P99_MS: %.3f\n", latency:percentile(99.0) / 1000))
end
