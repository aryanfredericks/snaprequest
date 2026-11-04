// The `snapreq` API of pre-request and test scripts (ADR-031). Pure JavaScript over the input
// document: nothing here reaches outside the engine. `__snapreq_run(code)` runs a script and
// returns the output document as JSON.
"use strict";
(function () {
  // Kept before user code runs, which could replace the globals.
  const stringify = JSON.stringify;
  const input = JSON.parse(globalThis.__snapreq_input);
  delete globalThis.__snapreq_input;

  const out = { vars: [], request: input.request, tests: [], logs: [], error: null };
  const vars = Object.create(null);
  for (const [k, v] of Object.entries(input.vars)) vars[k] = v;

  const MAX_LOGS = 1000;
  const MAX_LINE = 4000;
  const MAX_TESTS = 10000;

  const show = (v) => {
    if (typeof v === "string") return v;
    if (v === undefined) return "undefined";
    if (typeof v === "function") return "[function]";
    try {
      const s = JSON.stringify(v);
      return s === undefined ? String(v) : s;
    } catch (e) {
      return String(v);
    }
  };
  const brief = (v) => {
    const s = typeof v === "string" ? JSON.stringify(v) : show(v);
    return s.length > 200 ? s.slice(0, 200) + "…" : s;
  };
  const message = (e) => (e instanceof Error ? e.message : String(e));

  const console = {};
  for (const level of ["log", "info", "warn", "error", "debug"]) {
    console[level] = (...args) => {
      if (out.logs.length >= MAX_LOGS) return;
      let line = (level === "log" || level === "info" ? "" : level + ": ") + args.map(show).join(" ");
      if (line.length > MAX_LINE) line = line.slice(0, MAX_LINE) + "…";
      out.logs.push(line);
    };
  }

  const NAME = /^[A-Za-z0-9_.\-]+$/;
  const text = (v) => (typeof v === "string" ? v : show(v));
  const variables = Object.freeze({
    get(name) {
      return vars[name];
    },
    has(name) {
      return name in vars;
    },
    set(name, value) {
      setVar(name, value, false);
    },
    /** Like `set`, and the value is hidden in every later trace and report. */
    setSecret(name, value) {
      setVar(name, value, true);
    },
    toObject() {
      return Object.assign({}, vars);
    },
  });
  function setVar(name, value, secret) {
    name = String(name);
    if (!NAME.test(name)) throw new Error(`invalid variable name \`${name}\` (letters, digits, \`_\`, \`-\` and \`.\`)`);
    vars[name] = text(value);
    out.vars.push({ name, value: vars[name], secret });
  }

  function headerApi(list, writable) {
    const same = (h, name) => h.name.toLowerCase() === String(name).toLowerCase();
    const guard = () => {
      if (!writable) throw new Error("the response headers cannot be changed");
    };
    return Object.freeze({
      get(name) {
        const h = list.find((x) => same(x, name));
        return h ? h.value : undefined;
      },
      has(name) {
        return list.some((x) => same(x, name));
      },
      all() {
        return list.map((h) => ({ name: h.name, value: h.value }));
      },
      toObject() {
        const o = {};
        for (const h of list) o[h.name.toLowerCase()] = h.value;
        return o;
      },
      set(name, value) {
        guard();
        this.remove(name);
        list.push({ name: String(name), value: text(value) });
      },
      add(name, value) {
        guard();
        list.push({ name: String(name), value: text(value) });
      },
      remove(name) {
        guard();
        for (let i = list.length - 1; i >= 0; i--) if (same(list[i], name)) list.splice(i, 1);
      },
    });
  }

  const req = out.request;
  const pre = input.phase === "pre_request";
  const request = Object.freeze({
    get method() {
      return req.method;
    },
    set method(v) {
      if (!pre) throw new Error("the request can only be changed in a pre-request script");
      req.method = String(v).toUpperCase();
    },
    get url() {
      return req.url;
    },
    set url(v) {
      if (!pre) throw new Error("the request can only be changed in a pre-request script");
      req.url = String(v);
    },
    get body() {
      return req.body;
    },
    set body(v) {
      if (!pre) throw new Error("the request can only be changed in a pre-request script");
      req.body = v === null || v === undefined ? null : text(v);
    },
    headers: headerApi(req.headers, pre),
  });

  const res = input.response;
  const response = res
    ? Object.freeze({
        status: res.status,
        time: res.time_ms,
        headers: headerApi(res.headers.slice(), false),
        get body() {
          return res.body;
        },
        text() {
          return res.body;
        },
        json() {
          return JSON.parse(res.body);
        },
      })
    : undefined;

  class AssertionError extends Error {
    constructor(msg) {
      super(msg);
      this.name = "AssertionError";
    }
  }

  function deepEqual(a, b) {
    if (Object.is(a, b)) return true;
    if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    const ka = Object.keys(a);
    const kb = Object.keys(b);
    if (ka.length !== kb.length) return false;
    return ka.every((k) => Object.prototype.hasOwnProperty.call(b, k) && deepEqual(a[k], b[k]));
  }

  function typeOf(v) {
    if (v === null) return "null";
    if (Array.isArray(v)) return "array";
    return typeof v;
  }

  function lookup(obj, path) {
    const parts = Array.isArray(path) ? path : String(path).split(".");
    let cur = obj;
    for (const p of parts) {
      if (cur === null || cur === undefined || !Object.prototype.hasOwnProperty.call(Object(cur), p)) return { found: false };
      cur = cur[p];
    }
    return { found: true, value: cur };
  }

  class Expectation {
    constructor(actual, negate) {
      this.actual = actual;
      this.negate = negate;
    }
    check(pass, what) {
      if (pass === this.negate) {
        throw new AssertionError(`expected ${brief(this.actual)} ${this.negate ? "not " : ""}${what}`);
      }
      return this;
    }
    get not() {
      return new Expectation(this.actual, !this.negate);
    }
    // Jest style.
    toBe(v) {
      return this.check(Object.is(this.actual, v), `to be ${brief(v)}`);
    }
    toEqual(v) {
      return this.check(deepEqual(this.actual, v), `to equal ${brief(v)}`);
    }
    toContain(v) {
      const a = this.actual;
      const pass =
        typeof a === "string" ? a.includes(String(v)) : Array.isArray(a) ? a.some((x) => deepEqual(x, v)) : false;
      return this.check(pass, `to contain ${brief(v)}`);
    }
    toBeGreaterThan(n) {
      return this.check(this.actual > n, `to be greater than ${brief(n)}`);
    }
    toBeGreaterThanOrEqual(n) {
      return this.check(this.actual >= n, `to be at least ${brief(n)}`);
    }
    toBeLessThan(n) {
      return this.check(this.actual < n, `to be less than ${brief(n)}`);
    }
    toBeLessThanOrEqual(n) {
      return this.check(this.actual <= n, `to be at most ${brief(n)}`);
    }
    toMatch(re) {
      const r = re instanceof RegExp ? re : new RegExp(String(re));
      return this.check(typeof this.actual === "string" && r.test(this.actual), `to match ${r}`);
    }
    toBeTruthy() {
      return this.check(!!this.actual, "to be truthy");
    }
    toBeFalsy() {
      return this.check(!this.actual, "to be falsy");
    }
    toBeDefined() {
      return this.check(this.actual !== undefined, "to be defined");
    }
    toBeUndefined() {
      return this.check(this.actual === undefined, "to be undefined");
    }
    toBeNull() {
      return this.check(this.actual === null, "to be null");
    }
    toHaveProperty(path, ...value) {
      const r = lookup(this.actual, path);
      const pass = r.found && (value.length === 0 || deepEqual(r.value, value[0]));
      return this.check(pass, `to have property ${brief(path)}${value.length ? " " + brief(value[0]) : ""}`);
    }
    toHaveLength(n) {
      const len = this.actual === null || this.actual === undefined ? undefined : this.actual.length;
      return this.check(len === n, `to have length ${n}`);
    }
    toBeOneOf(list) {
      return this.check(list.some((x) => deepEqual(x, this.actual)), `to be one of ${brief(list)}`);
    }
    toBeTypeOf(type) {
      const t = type === "integer" ? (Number.isInteger(this.actual) ? "integer" : typeOf(this.actual)) : typeOf(this.actual);
      return this.check(t === type, `to be of type ${type}`);
    }
    // Chai style (BDD chains).
    get to() {
      return this;
    }
    get be() {
      return this;
    }
    get been() {
      return this;
    }
    get is() {
      return this;
    }
    get that() {
      return this;
    }
    get which() {
      return this;
    }
    get and() {
      return this;
    }
    get has() {
      return this;
    }
    get have() {
      return this;
    }
    get with() {
      return this;
    }
    get deep() {
      this.deepFlag = true;
      return this;
    }
    equal(v) {
      return this.deepFlag ? this.toEqual(v) : this.check(this.actual === v, `to equal ${brief(v)}`);
    }
    equals(v) {
      return this.equal(v);
    }
    eql(v) {
      return this.toEqual(v);
    }
    include(v) {
      const a = this.actual;
      if (a !== null && typeof a === "object" && !Array.isArray(a) && typeof v === "object") {
        return this.check(Object.keys(v).every((k) => deepEqual(a[k], v[k])), `to include ${brief(v)}`);
      }
      return this.toContain(v);
    }
    contain(v) {
      return this.include(v);
    }
    contains(v) {
      return this.include(v);
    }
    includes(v) {
      return this.include(v);
    }
    above(n) {
      return this.toBeGreaterThan(n);
    }
    gt(n) {
      return this.toBeGreaterThan(n);
    }
    below(n) {
      return this.toBeLessThan(n);
    }
    lt(n) {
      return this.toBeLessThan(n);
    }
    least(n) {
      return this.toBeGreaterThanOrEqual(n);
    }
    most(n) {
      return this.toBeLessThanOrEqual(n);
    }
    within(lo, hi) {
      return this.check(this.actual >= lo && this.actual <= hi, `to be within ${lo}..${hi}`);
    }
    match(re) {
      return this.toMatch(re);
    }
    property(name, ...value) {
      return this.toHaveProperty([String(name)], ...value);
    }
    lengthOf(n) {
      return this.toHaveLength(n);
    }
    length(n) {
      return this.toHaveLength(n);
    }
    oneOf(list) {
      return this.toBeOneOf(list);
    }
    a(type) {
      return this.toBeTypeOf(String(type).toLowerCase());
    }
    an(type) {
      return this.a(type);
    }
    get ok() {
      return this.toBeTruthy();
    }
    get true() {
      return this.check(this.actual === true, "to be true");
    }
    get false() {
      return this.check(this.actual === false, "to be false");
    }
    get null() {
      return this.toBeNull();
    }
    get undefined() {
      return this.toBeUndefined();
    }
    get exist() {
      return this.check(this.actual !== null && this.actual !== undefined, "to exist");
    }
    get empty() {
      const a = this.actual;
      const len = typeof a === "string" || Array.isArray(a) ? a.length : a && typeof a === "object" ? Object.keys(a).length : undefined;
      return this.check(len === 0, "to be empty");
    }
  }

  function expect(actual) {
    return new Expectation(actual, false);
  }

  function test(name, fn) {
    if (out.tests.length >= MAX_TESTS) return;
    name = String(name);
    try {
      const r = fn();
      if (r && typeof r.then === "function") throw new Error("asynchronous tests are not supported");
      out.tests.push({ name, passed: true, error: null });
    } catch (e) {
      out.tests.push({ name, passed: false, error: message(e) });
    }
  }

  const snapreq = Object.freeze({
    phase: input.phase,
    variables,
    request,
    response,
    test,
    expect,
  });

  globalThis.__snapreq_run = function (code) {
    try {
      const fn = new Function("snapreq", "console", '"use strict";\n' + code);
      fn.call(undefined, snapreq, console);
    } catch (e) {
      out.error = e instanceof Error ? `${e.name}: ${e.message}` : `uncaught ${show(e)}`;
    }
    return stringify(out);
  };
})();
