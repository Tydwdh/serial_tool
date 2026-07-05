local host = __test_host
local test = {
  _cases = {},
  _before_each = nil,
  _after_each = nil,
  _timeout_ms = 5000,
  _current = nil
}

function test.timeout(ms)
  test._timeout_ms = ms
end

function test.before_each(fn)
  test._before_each = fn
end

function test.after_each(fn)
  test._after_each = fn
end

function test.log(message)
  local text = tostring(message)
  if test._current then
    table.insert(test._current.logs, text)
  end
  if ctx.log then
    ctx.log.info(text)
  end
end

function test.assert(condition, message)
  if test._current then
    test._current.assertions = test._current.assertions + 1
  end
  if not condition then
    error(message or "assertion failed", 2)
  end
end

function test.expect(topic, timeout_ms)
  return ctx.bus.wait(topic, timeout_ms or test._timeout_ms)
end

local function publish_report()
  host.publish_report({
    run_id = host.run_id,
    source = host.source,
    script_name = host.script_name,
    started_ms = host.run_started_ms,
    finished_ms = host.now_ms(),
    cases = test._cases
  })
end

function test.case(name, fn)
  local start_event_id = host.latest_event_id()
  local started = host.now_ms()
  local case = {
    name = name,
    status = "passed",
    duration_ms = 0,
    logs = {},
    assertions = 0,
    error = nil,
    raw_packets = {}
  }

  test._current = case
  local ok, err = pcall(function()
    if test._before_each then test._before_each() end
    fn()
  end)

  local after_ok, after_err = true, nil
  if test._after_each then
    after_ok, after_err = pcall(test._after_each)
  end

  case.duration_ms = host.now_ms() - started
  case.raw_packets = host.raw_packets_since(start_event_id)

  if not ok then
    case.status = "failed"
    case.error = tostring(err)
  elseif not after_ok then
    case.status = "failed"
    case.error = tostring(after_err)
  end

  test._current = nil
  table.insert(test._cases, case)
  publish_report()
end

_G.test = test
