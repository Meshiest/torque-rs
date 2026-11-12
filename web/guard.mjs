// The browser checks never let a page take the pointer lock: headless Chrome on Windows confines the real cursor to its
// invisible window when a page locks the pointer, and a check that clicks the canvas would do exactly that.
//
// This file owns the browser. `launchGuarded` is the only way a script in this repository gets one: it launches puppeteer and
// installs, for every page and frame it opens (popups are blocked), a script that runs before the page's own and replaces
// `requestPointerLock` with a no-op that cannot be put back. Whatever URL a check loads, or however it navigates, no page
// can lock the pointer. Pages also load with `?nolock` (`noLock`), which `index.html` honors the same way.
// `guard-scan.mjs` fails any other file that names a browser driver or a browser program.
import { createRequire } from 'node:module';
import { extname, join, resolve } from 'node:path';

// Runs in the page, before any of its scripts. `requestPointerLock` (with or without `{ unadjustedMovement: true }`) is one
// method on `Element.prototype`; each prototype below gets its own non-writable, non-configurable no-op so that neither an
// assignment nor `defineProperty` on a body or canvas, nor on `Object` itself, brings the real one back.
function blockPointerLock() {
  const key = 'requestPointerLock';
  const noop = { [key]() { return Promise.resolve(); } }[key];
  const hide = (target) => {
    try {
      Object.defineProperty(target, key, { value: noop, writable: false, configurable: false, enumerable: false });
    } catch {}
  };
  for (const name of ['Element', 'HTMLElement', 'HTMLBodyElement', 'HTMLCanvasElement', 'HTMLDivElement']) {
    const type = globalThis[name];
    if (type) hide(type.prototype);
  }
  const refuse = (define) =>
    function (target, property, ...rest) {
      if (property === key) return define === Reflect.defineProperty ? false : target;
      return define.call(this, target, property, ...rest);
    };
  const refuseAll = (define) =>
    function (target, properties) {
      if (properties && Object.prototype.hasOwnProperty.call(Object(properties), key)) return target;
      return define.call(this, target, properties);
    };
  const lock = (owner, name, value) => {
    try {
      Object.defineProperty(owner, name, { value, writable: false, configurable: false });
    } catch {}
  };
  lock(Object, 'defineProperty', refuse(Object.defineProperty));
  lock(Reflect, 'defineProperty', refuse(Reflect.defineProperty));
  lock(Object, 'defineProperties', refuseAll(Object.defineProperties));
}
const script = `(${blockPointerLock.toString()})()`;

async function guardPage(page) {
  await page.evaluateOnNewDocument(script);
  // a page that is already open (a popup the browser made before this ran) gets it now, too.
  await page.evaluate(script).catch(() => {});
}

// Every way a page is made goes through `guardPage` first, before the caller can navigate it.
function guardContext(context) {
  const newPage = context.newPage.bind(context);
  context.newPage = async (...args) => {
    const page = await newPage(...args);
    await guardPage(page);
    return page;
  };
  return context;
}

/** `url` with the `nolock` option on its query. */
export function noLock(url) {
  const u = new URL(url);
  u.searchParams.set('nolock', '1');
  return u.href;
}

/**
 * Launches headless Chrome with puppeteer-core (from the folder `TORQUE_WEB_TOOLS` names; the program `CHROME` names, else the
 * installed stable channel) and returns the browser. Chrome's sandbox stays on unless `TORQUE_CHROME_NO_SANDBOX=1` is set (a
 * container that cannot run it; a hostile page then has the process's own rights). Every page it opens, in any context, has the pointer
 * lock replaced
 * before its own scripts run. `options` are puppeteer's launch options (`args`, `defaultViewport`); `headless` is always on.
 * With `browser: 'firefox'` it is headless Firefox instead (the program `FIREFOX` names, else the installed one), driven over
 * WebDriver BiDi in a fresh profile with Firefox's own defaults but for the guards: the pointer lock switched off in the
 * browser itself (and replaced in every page as in Chrome), and no popup allowed at all.
 * `prefs` are further Firefox preferences (a guard's preference is never overridden).
 */
export async function launchGuarded(options = {}) {
  if (options.headless === false) throw new Error('a guarded browser is always headless');
  const tools = resolve(process.env.TORQUE_WEB_TOOLS ?? '.');
  const puppeteer = createRequire(join(tools, 'package.json'))('puppeteer-core');
  if (options.browser === 'firefox') return launchFirefox(puppeteer, options);
  if (options.browser && options.browser !== 'chrome') throw new Error(`no guarded browser "${options.browser}"`);
  // No popups: `window.open` and `target=_blank` open nothing (a page made outside `newPage` could run a script before the
  // guard reaches it).
  const noSandbox = process.env.TORQUE_CHROME_NO_SANDBOX === '1' ? ['--no-sandbox'] : [];
  const args = [...(options.args ?? []), '--block-new-web-contents', ...noSandbox];
  const browser = await puppeteer.launch({
    ...(process.env.CHROME ? { executablePath: process.env.CHROME } : { channel: 'chrome' }),
    ...options,
    args,
    headless: true,
  });
  // the tab Chrome opens at launch
  await Promise.all((await browser.pages()).map(guardPage));
  guardContext(browser.defaultBrowserContext());
  const createContext = (browser.createBrowserContext ?? browser.createIncognitoBrowserContext).bind(browser);
  browser.createBrowserContext = browser.createIncognitoBrowserContext = async (...a) => guardContext(await createContext(...a));
  const newPage = browser.newPage.bind(browser);
  browser.newPage = async (...a) => {
    const page = await newPage(...a);
    await guardPage(page);
    return page;
  };
  // A page made some other way (a popup, which the flag does not always stop) is held at its start until the guard is in it:
  // the browser is told to wait for this session before running the new page's first script.
  const session = await browser.target().createCDPSession();
  session.on('sessionattached', async (child) => {
    try {
      await child.send('Page.addScriptToEvaluateOnNewDocument', { source: script, runImmediately: true });
    } catch {}
    await child.send('Runtime.runIfWaitingForDebugger').catch(() => {});
  });
  await session.send('Target.setAutoAttach', {
    autoAttach: true,
    waitForDebuggerOnStart: true,
    flatten: true,
    filter: [{ type: 'page' }, { exclude: true }],
  });
  // and as soon as it is known, should that not have caught it.
  browser.on('targetcreated', async (target) => {
    if (target.type() === 'page') {
      const page = await target.page().catch(() => null);
      if (page) await guardPage(page).catch(() => {});
    }
  });
  return browser;
}

// Firefox's guards, set last so no caller's preference undoes them.
const FIREFOX_GUARDS = Object.freeze({
  // no page can lock the pointer at all
  'full-screen-api.pointer-lock.enabled': false,
  // no popups: the blocker on (puppeteer's profile turns it off), no event lets one through, and none may be open
  'dom.disable_open_during_load': true,
  'dom.popup_allowed_events': '',
  'dom.disable_window_flip': true,
  'dom.popup_maximum': 0,
});

// Where Firefox's installer puts it (on Windows with the file extension Node's own program has).
const INSTALLED_FIREFOX = {
  win32: join(process.env.ProgramFiles ?? 'C:/Program Files', 'Mozilla Firefox', `firefox${extname(process.execPath)}`),
  darwin: '/Applications/Firefox.app/Contents/MacOS/firefox',
  linux: '/usr/bin/firefox',
};

async function launchFirefox(puppeteer, { browser: _, prefs = {}, args = [], ...options }) {
  const browser = await puppeteer.launch({
    executablePath: process.env.FIREFOX ?? INSTALLED_FIREFOX[process.platform] ?? 'firefox',
    ...options,
    browser: 'firefox',
    protocol: 'webDriverBiDi',
    args: [...args],
    extraPrefsFirefox: { ...prefs, ...FIREFOX_GUARDS },
    headless: true,
    // A browser that crashes, or is ended on close, never opens the crash reporter's window on the desktop.
    env: { ...process.env, ...options.env, MOZ_CRASHREPORTER_DISABLE: '1', MOZ_CRASHREPORTER_NO_REPORT: '1' },
  });
  await Promise.all((await browser.pages()).map(guardPage));
  guardContext(browser.defaultBrowserContext());
  const createContext = browser.createBrowserContext.bind(browser);
  browser.createBrowserContext = async (...a) => guardContext(await createContext(...a));
  const newPage = browser.newPage.bind(browser);
  browser.newPage = async (...a) => {
    const page = await newPage(...a);
    await guardPage(page);
    return page;
  };
  browser.on('targetcreated', async (target) => {
    if (target.type() === 'page') {
      const page = await target.page().catch(() => null);
      if (page) await guardPage(page).catch(() => {});
    }
  });
  return browser;
}
