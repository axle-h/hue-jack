/*
 * Innertube support worker: mints PO tokens with BotGuard and runs YouTube's player script (signature and
 * n-parameter deciphering). Both execute code downloaded from YouTube and BotGuard needs browser globals
 * (jsdom), so they run in this child process instead of the sidecar itself.
 *
 * The PO token minter is adapted from volumio-yt-support (https://github.com/patrickkfkan/volumio-yt-support,
 * src/lib/innertube/PoToken.ts), Copyright (c) Patrick Kan, MIT licence, which is in turn based on the BgUtils
 * example (https://github.com/LuanRT/BgUtils, MIT).
 */
import { BotGuardClient } from 'bgutils-js/botguard';
import { buildURL, GOOG_API_KEY, USER_AGENT } from 'bgutils-js/utils';
import { WebPoMinter } from 'bgutils-js/webpo';
import { JSDOM } from 'jsdom';
import type { WorkerReply, WorkerRequest } from './protocol.js';

interface BgChallenge {
  interpreter_url: { private_do_not_access_or_else_trusted_resource_url_wrapped_value: string };
  program: string;
  global_name: string;
}

interface Minter {
  minter: WebPoMinter;
  ttl: number;
  refreshThreshold: number;
  created: number;
}

let challenge: BgChallenge | null = null;
let minterPromise: Promise<Minter> | null = null;

async function installBrowserGlobals() {
  const dom = new JSDOM('<!DOCTYPE html><html lang="en"><head><title></title></head><body></body></html>', {
    url: 'https://www.youtube.com/',
    referrer: 'https://www.youtube.com/',
    pretendToBeVisual: true,
  });
  // jsdom has no canvas; borrow happy-dom's canvas mock, which BotGuard probes.
  try {
    const { Window } = await import('happy-dom');
    const happyWindow = new Window();
    const happyCanvasProto = Object.getPrototypeOf(happyWindow.document.createElement('canvas'));
    (dom.window.HTMLCanvasElement.prototype as unknown as { getContext: unknown }).getContext = happyCanvasProto.getContext;
  } catch {
    // Without the canvas mock BotGuard usually still works.
  }
  Object.assign(globalThis, {
    window: dom.window,
    document: dom.window.document,
    location: dom.window.location,
    origin: dom.window.origin,
  });
  if (!Reflect.has(globalThis, 'navigator')) {
    Object.defineProperty(globalThis, 'navigator', { value: dom.window.navigator });
  }
}

async function createMinter(bg: BgChallenge): Promise<Minter> {
  await installBrowserGlobals();
  const interpreterUrl = bg.interpreter_url.private_do_not_access_or_else_trusted_resource_url_wrapped_value;
  const interpreter = await (await fetch(`https:${interpreterUrl}`)).text();
  if (!interpreter) {
    throw new Error('Could not load the BotGuard VM');
  }
  new Function(interpreter)();

  const botguard = await BotGuardClient.create({
    program: bg.program,
    globalName: bg.global_name,
    globalObject: globalThis,
  });
  const webPoSignalOutput: Parameters<typeof WebPoMinter.create>[1] = [];
  const botguardResponse = await botguard.snapshot({ webPoSignalOutput });
  const requestKey = 'O43z0dpjhgX20SCx4KAo';
  const res = await fetch(buildURL('GenerateIT', true), {
    method: 'POST',
    headers: {
      'content-type': 'application/json+protobuf',
      'x-goog-api-key': GOOG_API_KEY,
      'x-user-agent': 'grpc-web-javascript/0.1',
      'user-agent': USER_AGENT,
    },
    body: JSON.stringify([requestKey, botguardResponse]),
  });
  const response = (await res.json()) as unknown[];
  if (typeof response[0] !== 'string') {
    throw new Error('Could not get an integrity token');
  }
  const [integrityToken, ttl, refreshThreshold] = response as [string, number, number];
  const minter = await WebPoMinter.create({ integrityToken }, webPoSignalOutput);
  return { minter, ttl: ttl ?? 3600, refreshThreshold: refreshThreshold ?? 100, created: Date.now() };
}

function getMinter(): Promise<Minter> {
  if (!challenge) {
    return Promise.reject(new Error('Worker not initialised with a BotGuard challenge'));
  }
  if (minterPromise) {
    const current = minterPromise;
    return current.then((m) => {
      const ageSecs = (Date.now() - m.created) / 1000;
      if (ageSecs > m.ttl - m.refreshThreshold && minterPromise === current) {
        minterPromise = createMinter(challenge!);
        minterPromise.catch(() => (minterPromise = null));
      }
      return m;
    });
  }
  minterPromise = createMinter(challenge);
  minterPromise.catch(() => (minterPromise = null));
  return minterPromise;
}

async function handle(req: WorkerRequest): Promise<unknown> {
  switch (req.type) {
    case 'init':
      challenge = req.challenge as BgChallenge;
      minterPromise = null;
      return true;
    case 'pot': {
      const { minter, ttl, created } = await getMinter();
      const poToken = await minter.mintAsWebsafeString(req.identifier);
      return { poToken, ttl: Math.floor((ttl * 1000 + created - Date.now()) / 1000) };
    }
    case 'run':
      // youtubei.js appends the call and `return` statement to the extracted player functions.
      return new Function(req.data.output)();
    default:
      throw new Error(`Unknown request ${(req as { type: string }).type}`);
  }
}

process.on('message', (req: WorkerRequest) => {
  handle(req).then(
    (result) => process.send?.({ id: req.id, ok: true, result } satisfies WorkerReply),
    (err: unknown) =>
      process.send?.({ id: req.id, ok: false, error: err instanceof Error ? err.message : String(err) } satisfies WorkerReply),
  );
});

process.on('disconnect', () => process.exit(0));
