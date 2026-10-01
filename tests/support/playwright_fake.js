// A fake `playwright` package for Botwork's tests: the API surface Botwork's
// host uses, with no browser. Every call is appended to calls.log in the
// working directory. The page knows a few selectors:
//   #status   says "Saving" until 200 ms after the page loaded, then "Saved"
//   #late     appears 300 ms after the page loaded
//   #never    never appears
//   anything else is one visible element whose text names it.
const fs = require("node:fs");
const { spawn } = require("node:child_process");

function log(...call) {
  fs.appendFileSync("calls.log", `${JSON.stringify(call)}\n`);
}

class TimeoutError extends Error {
  constructor(message) {
    super(message);
    this.name = "TimeoutError";
  }
}

class Locator {
  constructor(page, selector) {
    this.page = page;
    this.selector = selector;
  }
  first() {
    return this;
  }
  present() {
    const since = Date.now() - this.page.loaded;
    if (this.selector === "#never") return false;
    if (this.selector === "#late") return since >= 300;
    return true;
  }
  text() {
    if (this.selector === "#status") {
      return Date.now() - this.page.loaded >= 200 ? "Saved" : "Saving";
    }
    return `text of ${this.selector}`;
  }
  async click() {
    log("click", this.selector);
  }
  async fill(text) {
    log("fill", this.selector, text);
  }
  async press(key) {
    log("press", this.selector, key);
  }
  async setChecked(checked) {
    log("setChecked", this.selector, checked);
  }
  async hover() {
    log("hover", this.selector);
  }
  async selectOption(values) {
    log("selectOption", this.selector, values);
    return Array.isArray(values) ? values : [values];
  }
  async innerText() {
    log("innerText", this.selector);
    if (!this.present()) throw new TimeoutError(`waiting for ${this.selector}`);
    return this.text();
  }
  async getAttribute(name) {
    return name === "missing" ? null : `${name} of ${this.selector}`;
  }
  async count() {
    return this.present() ? 2 : 0;
  }
  async isVisible() {
    return this.present();
  }
  async waitFor({ timeout }) {
    const deadline = Date.now() + timeout;
    while (!this.present()) {
      if (Date.now() >= deadline) throw new TimeoutError(`Timeout ${timeout}ms exceeded`);
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
  }
}

class Page {
  constructor(context) {
    this.context = context;
    this.address = "about:blank";
    this.loaded = Date.now();
  }
  async goto(url) {
    log("goto", url);
    if (url === "http://fail.test/") throw new Error("page.goto: net::ERR_NAME_NOT_RESOLVED at http://fail.test/");
    this.address = url;
    this.loaded = Date.now();
  }
  url() {
    return this.address;
  }
  async title() {
    return "Fake page";
  }
  locator(selector) {
    return new Locator(this, selector);
  }
  async evaluate(callback, argument) {
    const [script, values] = argument;
    log("evaluate", script, values);
    if (script === "huge") return 2 ** 31 + 1;
    return values;
  }
  async screenshot({ path, fullPage }) {
    log("screenshot", fullPage);
    fs.writeFileSync(path, "png");
  }
  video() {
    return this.context.settings.recordVideo
      ? { path: async () => `${this.context.settings.recordVideo.dir}/page.webm` }
      : null;
  }
  async close() {
    log("closePage");
  }
}

class Context {
  constructor(settings) {
    this.settings = settings;
    this.tracing = {
      start: async (options) => log("tracing.start", options),
      stop: async ({ path }) => {
        log("tracing.stop");
        fs.writeFileSync(path, "zip");
      },
    };
  }
  setDefaultTimeout(timeout) {
    log("setDefaultTimeout", timeout);
  }
  async newPage() {
    return new Page(this);
  }
  async close() {
    log("closeContext");
    if (this.settings.recordVideo) {
      fs.mkdirSync(this.settings.recordVideo.dir, { recursive: true });
      fs.writeFileSync(`${this.settings.recordVideo.dir}/page.webm`, "webm");
    }
  }
}

class Browser {
  constructor(options) {
    this.options = options;
    // A child standing in for the browser's own processes.
    if ((options.args ?? []).includes("--child")) {
      const child = spawn(process.execPath, ["-e", "setTimeout(() => {}, 60000)"], { stdio: "ignore" });
      fs.writeFileSync("browser.pid", String(child.pid));
    }
  }
  version() {
    return "1.0-fake";
  }
  async newContext(settings) {
    log("newContext", settings);
    return new Context(settings);
  }
  async close() {
    log("closeBrowser");
  }
}

function type(name) {
  return {
    async launch(options) {
      log("launch", name, options);
      return new Browser(options);
    },
  };
}

module.exports = { chromium: type("chromium"), firefox: type("firefox"), webkit: type("webkit") };
