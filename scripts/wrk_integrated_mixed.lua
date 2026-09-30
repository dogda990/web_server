-- 70% light, 20% delayed GET, and 10% delayed 10 MiB POST requests.
local sequence = 0
local payload = string.rep("x", 10 * 1024 * 1024)

request = function()
  sequence = sequence + 1
  local slot = (sequence - 1) % 10
  if slot < 7 then
    wrk.method = "GET"
    wrk.headers["Content-Type"] = nil
    wrk.body = nil
    return wrk.format(nil, "/rps_plain")
  elseif slot < 9 then
    wrk.method = "GET"
    wrk.headers["Content-Type"] = nil
    wrk.body = nil
    return wrk.format(nil, "/rps_heavy")
  end

  wrk.method = "POST"
  wrk.headers["Content-Type"] = "application/octet-stream"
  wrk.body = payload
  return wrk.format(nil, "/rps_heavy")
end

done = function(summary, latency, requests)
  io.write(string.format("CUSTOM_P95_MS: %.3f\n", latency:percentile(95.0) / 1000))
  io.write(string.format("CUSTOM_P99_MS: %.3f\n", latency:percentile(99.0) / 1000))
end
