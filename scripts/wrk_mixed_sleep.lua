-- Repeat a 70% light / 30% 25 ms heavy request mix.
local sequence = 0

request = function()
  sequence = sequence + 1
  local slot = (sequence - 1) % 10
  if slot < 7 then
    wrk.method = "GET"
    wrk.headers["Content-Type"] = nil
    wrk.body = nil
    return wrk.format(nil, "/rps_plain")
  end

  wrk.method = "GET"
  wrk.headers["Content-Type"] = nil
  wrk.body = nil
  return wrk.format(nil, "/rps_heavy")
end

done = function(summary, latency, requests)
  io.write(string.format("CUSTOM_P95_MS: %.3f\n", latency:percentile(95.0) / 1000))
  io.write(string.format("CUSTOM_P99_MS: %.3f\n", latency:percentile(99.0) / 1000))
end
