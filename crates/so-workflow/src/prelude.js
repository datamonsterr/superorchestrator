// Installed once per run, before the script. Wraps the three native hooks
// (__begin, __run, __event) into the public API and removes them from the globals.
(function (g) {
  'use strict';
  const begin = g.__begin, run = g.__run, event = g.__event;
  delete g.__begin; delete g.__run; delete g.__event;
  const argsJson = g.__args_json;
  delete g.__args_json;

  const MAX_ITEMS = 4096;
  const pending = [];
  let parDepth = 0;
  let curPhase = '';

  const track = (p) => { pending.push(p.then(() => {}, () => {})); return p; };
  const snake = (k) => k.replace(/[A-Z]/g, (c) => '_' + c.toLowerCase());
  const ALIAS = { schema: 'output_schema', label: 'name' };

  function normOpts(opts) {
    if (opts === undefined || opts === null) return {};
    if (typeof opts !== 'object' || Array.isArray(opts)) {
      throw new TypeError('agent() options must be an object');
    }
    const out = {};
    for (const k of Object.keys(opts)) {
      if (opts[k] === undefined) continue;
      const key = ALIAS[k] || snake(k);
      out[key] = opts[k];
    }
    return out;
  }

  function agentFailure(msg) {
    const e = new Error(msg);
    e.agentFailed = true;
    return e;
  }

  // Everything up to the first await runs synchronously at call time, which is what
  // fixes the step order and the parallel() isolation default.
  function callAgent(prompt, opts, forceWorktree) {
    let ticket;
    try {
      const o = normOpts(opts);
      if (forceWorktree && o.isolation === undefined) o.isolation = 'worktree';
      ticket = JSON.parse(begin(String(prompt), JSON.stringify(o), curPhase));
    } catch (e) {
      return Promise.reject(e);
    }
    if (ticket.err !== undefined) return Promise.reject(new Error(ticket.err));
    return run(ticket.ok).then((raw) => {
      const r = JSON.parse(raw);
      if (r.err !== undefined) {
        throw r.fatal ? new Error(r.err) : agentFailure(r.err);
      }
      return r.ok;
    });
  }

  const nullIfAgentFailed = (e) => {
    if (e && e.agentFailed) return null;
    throw e;
  };
  const start = (fn, ...a) => {
    try { return Promise.resolve(fn(...a)); } catch (e) { return Promise.reject(e); }
  };
  const checkList = (name, items) => {
    if (!Array.isArray(items)) throw new TypeError(name + '() expects an array');
    if (items.length > MAX_ITEMS) {
      throw new RangeError(name + '(): at most ' + MAX_ITEMS + ' items');
    }
  };

  const worktreeAgent = (prompt, opts) => callAgent(prompt, opts, true);

  g.agent = (prompt, opts) => callAgent(prompt, opts, parDepth > 0);

  // Items: functions (called with a worktree-defaulting agent), or promises/values.
  g.parallel = async (items) => {
    checkList('parallel', items);
    const starts = items.map((it) => {
      if (typeof it !== 'function') return Promise.resolve(it);
      parDepth++;
      try { return start(it, worktreeAgent); } finally { parDepth--; }
    });
    return Promise.all(starts.map((p) => p.catch(nullIfAgentFailed)));
  };

  // pipeline(items, fn): fn per item, concurrently (Claude Code form).
  // pipeline([fn, ...]): stages in order, each given the previous result.
  g.pipeline = async (items, fn) => {
    checkList('pipeline', items);
    if (typeof fn === 'function') {
      return Promise.all(items.map((it, i) => start(fn, it, i).catch(nullIfAgentFailed)));
    }
    let prev;
    for (const stage of items) {
      prev = typeof stage === 'function' ? await stage(prev) : await stage;
    }
    return prev;
  };

  g.phase = async (name, fn) => {
    const prev = curPhase;
    const n = String(name);
    await event('phase_started', n);
    curPhase = n;
    try {
      return await fn();
    } finally {
      curPhase = prev;
      await event('phase_finished', n);
    }
  };

  g.log = (msg) => { track(event('log', String(msg))); };

  g.args = argsJson === 'null' ? undefined : JSON.parse(argsJson);

  // Resume replays the same agent() calls, so scripts must be deterministic.
  const banned = (what) => () => {
    throw new Error(what + ' is disabled in workflow scripts; pass it in through args');
  };
  Date.now = banned('Date.now()');
  Math.random = banned('Math.random()');

  g.__drain = () => Promise.all(pending);
})(globalThis);
