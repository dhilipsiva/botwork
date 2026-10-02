// The Node side of Botwork's Playwright statements (decision D11). Botwork runs
// one per run, as `node --input-type=module -e <this file>`, in the run's
// directory, and keeps it for the run. Each line on stdin is a command,
//   {"id": 1, "command": "goto", "args": [...]},
// and each line on stdout an answer to one, in any order:
//   {"id": 1, "value": ...} or {"id": 1, "error": {"kind": ..., "message": ...}}.
// When stdin ends, the host closes every browser it launched and exits.
import { createRequire } from "node:module";
import { randomUUID } from "node:crypto";
import { createInterface } from "node:readline";
import path from "node:path";
import { writeFile } from "node:fs/promises";

// Stdout carries answers alone; anything else written there goes to stderr.
const respond = process.stdout.write.bind(process.stdout);
process.stdout.write = process.stderr.write.bind(process.stderr);

// The run's own `playwright`, resolved from its directory as `require` would.
let playwright;
let missing;
try {
  playwright = createRequire(path.join(process.cwd(), "botwork-playwright.cjs"))("playwright");
} catch (error) {
  missing = error;
}

// Handles are `{playwright: kind, id}`; IDs start with this host's own prefix,
// so a handle from another run's host never matches.
const prefix = randomUUID().slice(0, 8);
let counter = 0;
const objects = new Map();

class Failure extends Error {
  constructor(kind, message) {
    super(message);
    this.kind = kind;
  }
}

function store(kind, value, extra = {}) {
  const id = `${prefix}-${kind[0]}${++counter}`;
  objects.set(id, { kind, value, ...extra });
  return { playwright: kind, id };
}

// Forget `id` and everything it owns: a browser its contexts, a context its pages.
function forget(id) {
  objects.delete(id);
  for (const [other, entry] of [...objects.entries()]) {
    if (entry.owner === id) forget(other);
  }
}

function find(handle, kind) {
  const entry = handle && typeof handle === "object" ? objects.get(handle.id) : undefined;
  if (!entry || handle.playwright !== kind || entry.kind !== kind) {
    throw new Failure("handle", `not a ${kind} this run has open: it was closed, or another run opened it`);
  }
  return entry;
}

const BROWSERS = ["chromium", "firefox", "webkit"];

// Web-first checks: look again until `check` holds or the time is up.
async function until(timeout, check) {
  const deadline = Date.now() + timeout;
  let last;
  for (;;) {
    last = await check();
    if (last.ok) return last;
    if (Date.now() >= deadline) return last;
    await new Promise((resolve) => setTimeout(resolve, Math.min(100, Math.max(0, deadline - Date.now()))));
  }
}

function locator(page, selector) {
  return find(page, "page").value.locator(selector).first();
}

function timeoutOf(page) {
  return find(page, "page").timeout;
}

const commands = {
  async launch(options) {
    if (missing) {
      throw new Failure(
        "setup",
        "the `playwright` package is not installed where the run is; install it with `npm install playwright` and its browsers with `npx playwright install`",
      );
    }
    const {
      browser = "chromium",
      headless = true,
      args,
      executable_path,
      timeout_ms = 30000,
      // An absolute directory Botwork made, where failures in its pages are captured.
      failure_artifacts,
    } = options;
    if (!BROWSERS.includes(browser)) {
      throw new Failure("value", `Unknown browser \`${browser}\`; use chromium, firefox, or webkit`);
    }
    const launched = await playwright[browser].launch({
      headless,
      ...(args ? { args } : {}),
      ...(executable_path ? { executablePath: executable_path } : {}),
    });
    return {
      ...store("browser", launched, { timeout: timeout_ms, artifacts: failure_artifacts }),
      browser,
      version: launched.version(),
    };
  },
  async closeBrowser(handle) {
    const entry = find(handle, "browser");
    forget(handle.id);
    await entry.value.close();
    return null;
  },
  async newContext(handle, options) {
    const browser = find(handle, "browser");
    const { trace = false, ...settings } = options;
    const context = await browser.value.newContext(settings);
    context.setDefaultTimeout(browser.timeout);
    if (trace) {
      await context.tracing.start({ screenshots: true, snapshots: true });
    }
    return store("context", context, {
      timeout: browser.timeout,
      artifacts: browser.artifacts,
      owner: handle.id,
      pages: [],
    });
  },
  async closeContext(handle) {
    const entry = find(handle, "context");
    forget(handle.id);
    const videos = entry.pages.map((page) => page.video()).filter(Boolean);
    await entry.value.close();
    return Promise.all(videos.map((video) => video.path()));
  },
  async newPage(handle) {
    const context = find(handle, "context");
    const page = await context.value.newPage();
    context.pages.push(page);
    return store("page", page, { timeout: context.timeout, artifacts: context.artifacts, owner: handle.id });
  },
  async closePage(handle) {
    const entry = find(handle, "page");
    objects.delete(handle.id);
    await entry.value.close();
    return null;
  },
  async goto(handle, url) {
    await find(handle, "page").value.goto(url);
    return null;
  },
  async url(handle) {
    return find(handle, "page").value.url();
  },
  async title(handle) {
    return find(handle, "page").value.title();
  },
  async click(handle, selector) {
    await locator(handle, selector).click();
    return null;
  },
  async fill(handle, selector, text) {
    await locator(handle, selector).fill(text);
    return null;
  },
  async press(handle, selector, key) {
    await locator(handle, selector).press(key);
    return null;
  },
  async check(handle, selector, checked) {
    await locator(handle, selector).setChecked(checked);
    return null;
  },
  async hover(handle, selector) {
    await locator(handle, selector).hover();
    return null;
  },
  async select(handle, selector, value) {
    return locator(handle, selector).selectOption(value);
  },
  async text(handle, selector) {
    return locator(handle, selector).innerText();
  },
  async attribute(handle, selector, name) {
    return locator(handle, selector).getAttribute(name);
  },
  async count(handle, selector) {
    return find(handle, "page").value.locator(selector).count();
  },
  async visible(handle, selector) {
    return locator(handle, selector).isVisible();
  },
  async wait(handle, selector, timeout) {
    try {
      await locator(handle, selector).waitFor({ state: "visible", timeout });
    } catch (error) {
      if (error?.name === "TimeoutError") {
        throw new Failure("wait", `no element matched \`${selector}\` within ${timeout} ms`);
      }
      throw error;
    }
    return null;
  },
  async expectText(handle, selector, expected) {
    const target = locator(handle, selector);
    const result = await until(timeoutOf(handle), async () => {
      const actual = await target.innerText({ timeout: 100 }).catch(() => null);
      return { ok: actual === expected, actual };
    });
    if (!result.ok) {
      throw new Failure(
        "assertion",
        `\`${selector}\` should have the text ${JSON.stringify(expected)}, but ${result.actual === null ? "no element matched" : `it has ${JSON.stringify(result.actual)}`}`,
      );
    }
    return null;
  },
  async expectVisible(handle, selector) {
    const target = locator(handle, selector);
    const result = await until(timeoutOf(handle), async () => ({ ok: await target.isVisible() }));
    if (!result.ok) {
      throw new Failure("assertion", `\`${selector}\` should be visible, but it is not`);
    }
    return null;
  },
  async expectTitle(handle, expected) {
    const page = find(handle, "page").value;
    const result = await until(timeoutOf(handle), async () => {
      const actual = await page.title();
      return { ok: actual === expected, actual };
    });
    if (!result.ok) {
      throw new Failure("assertion", `the page should have the title ${JSON.stringify(expected)}, but it has ${JSON.stringify(result.actual)}`);
    }
    return null;
  },
  // The script is the Botwork script's own, run by design in the page, which
  // Playwright serializes this callback into: not in this Node process.
  async evaluate(handle, script, args) {
    return find(handle, "page").value.evaluate(
      ([body, values]) => new Function(body)(...values),
      [script, args],
    );
  },
  async screenshot(handle, file, fullPage) {
    await find(handle, "page").value.screenshot({ path: file, fullPage });
    return file;
  },
  async startTracing(handle) {
    await find(handle, "context").value.tracing.start({ screenshots: true, snapshots: true });
    return null;
  },
  async stopTracing(handle, file) {
    await find(handle, "context").value.tracing.stop({ path: file });
    return file;
  },
};

// Save what a page shows, when its browser keeps failure artifacts: a
// screenshot and its HTML, best effort, as the run's artifacts.
let captures = 0;
async function capture(id) {
  const entry = objects.get(id);
  if (!entry || entry.kind !== "page" || !entry.artifacts || entry.value.isClosed()) return [];
  const stem = path.join(entry.artifacts, `page-${id}-${++captures}`);
  const saved = [];
  try {
    await entry.value.screenshot({ path: `${stem}.png`, timeout: 5000 });
    saved.push({ kind: "failure-screenshot", path: `${stem}.png` });
  } catch {}
  try {
    await writeFile(`${stem}.html`, await entry.value.content());
    saved.push({ kind: "failure-source", path: `${stem}.html` });
  } catch {}
  return saved;
}

function answer(id, outcome) {
  respond(`${JSON.stringify({ id, ...outcome })}\n`);
}

const lines = createInterface({ input: process.stdin });
lines.on("line", (line) => {
  let request;
  try {
    request = JSON.parse(line);
  } catch {
    return;
  }
  const { id, command, args = [] } = request;
  const implementation = commands[command];
  if (!implementation) {
    answer(id, { error: { kind: "protocol", message: `unknown command ${command}` } });
    return;
  }
  Promise.resolve()
    .then(() => implementation(...args))
    .then(
      (value) => answer(id, { value: value === undefined ? null : value }),
      async (error) => {
        const kind = error instanceof Failure ? error.kind : error?.name === "TimeoutError" ? "timeout" : "error";
        // A command on a page captures it, unless the handle was wrong.
        const page = args[0]?.playwright === "page" && kind !== "handle" ? args[0].id : undefined;
        answer(id, {
          error: {
            kind,
            // Playwright colours its call logs for terminals.
            message: String(error?.message ?? error).replace(/\x1b\[[0-9;]*m/g, ""),
            artifacts: page ? await capture(page) : [],
          },
        });
      },
    );
});
lines.on("close", async () => {
  // The run ended with these pages open: capture the ones that keep failure
  // artifacts before their browsers close.
  const pages = [...objects.entries()].filter(([, entry]) => entry.kind === "page" && entry.artifacts);
  const saved = (await Promise.all(pages.map(([id]) => capture(id)))).flat();
  if (saved.length) {
    // Written before the host exits, as pipes on Windows write later.
    await new Promise((resolve) => respond(`${JSON.stringify({ artifacts: saved })}\n`, resolve));
  }
  const browsers = [...objects.values()].filter((entry) => entry.kind === "browser");
  await Promise.allSettled(browsers.map((entry) => entry.value.close()));
  process.exit(0);
});
