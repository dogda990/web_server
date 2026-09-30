done = function(summary, latency, requests)
  -- wrk latency percentiles are in microseconds; report milliseconds.
  io.write(string.format("CUSTOM_P95_MS: %.3f\n", latency:percentile(95.0) / 1000))
  io.write(string.format("CUSTOM_P99_MS: %.3f\n", latency:percentile(99.0) / 1000))
end
