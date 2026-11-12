// Drives the harness in headless Chrome over CDP. Each argument is a step:
//   open:<query>   load /harness/?<query> and wait until it's ready
//   wait:<ms>      sleep
//   eval:<js>      evaluate (awaiting promises) and print the result
//   key:<key>      press a key, e.g. key:j
//   shot:<path>    save a PNG screenshot
//   png:<sel>,<path> save the element matching <sel> as a PNG with a transparent background
//   hover:<sel>    move the mouse over the element matching <sel>
//   drag:<sel>,<dy>[,<dx>] drag the element matching <sel> by dy (and dx) px
// Needs `pnpm dev` running. Example:
//   node harness/drive.mjs open:fixture=huge wait:500 "eval:bench()" shot:/tmp/a.png
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";

const CHROME = process.env.CHROME ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const PORT = 9333;
const BASE = process.env.HARNESS_URL ?? "http://localhost:1420/harness/";

const chrome = spawn(CHROME, [
  "--headless=new",
  `--remote-debugging-port=${PORT}`,
  `--user-data-dir=/tmp/prism-harness-chrome`,
  "--window-size=1440,900",
  `--force-device-scale-factor=${process.env.DSF ?? 2}`,
  "about:blank",
]);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

let target;
for (let i = 0; i < 150 && !target; i++) {
  await sleep(100);
  try {
    target = (await (await fetch(`http://127.0.0.1:${PORT}/json/list`)).json()).find((t) => t.type === "page");
  } catch {}
}
if (!target) throw new Error("Chrome didn't start");

const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r));
let nextId = 0;
const pending = new Map();
ws.addEventListener("message", (e) => {
  const msg = JSON.parse(e.data);
  if (msg.method === "Runtime.consoleAPICalled" && msg.params.type === "error") {
    console.log("console.error:", msg.params.args.map((a) => a.value ?? a.description).join(" "));
  }
  if (msg.method === "Runtime.exceptionThrown") console.log("exception:", msg.params.exceptionDetails.exception?.description);
  if (msg.id != null) pending.get(msg.id)?.(msg);
});
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = ++nextId;
    pending.set(id, (m) => (m.error ? reject(new Error(`${method}: ${m.error.message}`)) : resolve(m.result)));
    ws.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => {
  const r = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  return r.result.value;
};

await send("Runtime.enable");
await send("Page.enable");
try {
  for (const step of process.argv.slice(2)) {
    const [cmd, ...rest] = step.split(":");
    const arg = rest.join(":");
    if (cmd === "open") {
      await send("Page.navigate", { url: `${BASE}?${arg}` });
      for (let i = 0; i < 300; i++) {
        await sleep(100);
        if (await evaluate("window.__ready === true").catch(() => false)) break;
      }
    } else if (cmd === "wait") await sleep(Number(arg));
    else if (cmd === "eval") console.log(`${arg} →`, JSON.stringify(await evaluate(arg)));
    else if (cmd === "key") {
      const named = { Enter: [13, "\r"], Escape: [27, undefined], ArrowDown: [40, undefined], ArrowUp: [38, undefined] }[arg];
      const [code, text] = named ?? [undefined, arg];
      for (const type of ["keyDown", "keyUp"]) {
        await send("Input.dispatchKeyEvent", { type, key: arg, windowsVirtualKeyCode: code, text: type === "keyDown" ? text : undefined });
      }
    } else if (cmd === "hover") {
      const r = await evaluate(`(() => { const b = document.querySelector(${JSON.stringify(arg)}).getBoundingClientRect(); return [b.x + b.width / 2, b.y + b.height / 2]; })()`);
      await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: r[0], y: r[1] });
    } else if (cmd === "drag") {
      const [sel, dy, dx = "0"] = arg.split(",");
      const r = await evaluate(`(() => { const b = document.querySelector(${JSON.stringify(sel)}).getBoundingClientRect(); return [b.x + b.width / 2, b.y + b.height / 2]; })()`);
      const [x, y] = r;
      await send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
      for (let i = 1; i <= 10; i++) {
        await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: x + (Number(dx) * i) / 10, y: y + (Number(dy) * i) / 10, button: "left" });
      }
      await send("Input.dispatchMouseEvent", { type: "mouseReleased", x: x + Number(dx), y: y + Number(dy), button: "left", clickCount: 1 });
    } else if (cmd === "png") {
      const [sel, path] = arg.split(",");
      const b = await evaluate(`(() => { const r = document.querySelector(${JSON.stringify(sel)}).getBoundingClientRect(); return [r.x, r.y, r.width, r.height]; })()`);
      await send("Emulation.setDefaultBackgroundColorOverride", { color: { r: 0, g: 0, b: 0, a: 0 } });
      const clip = { x: b[0], y: b[1], width: b[2], height: b[3], scale: 1 };
      const { data } = await send("Page.captureScreenshot", { format: "png", clip, captureBeyondViewport: true });
      writeFileSync(path, Buffer.from(data, "base64"));
      console.log(`saved ${path}`);
    } else if (cmd === "shot") {
      const { data } = await send("Page.captureScreenshot", { format: "png" });
      writeFileSync(arg, Buffer.from(data, "base64"));
      console.log(`saved ${arg}`);
    } else throw new Error(`unknown step ${step}`);
  }
} finally {
  ws.close();
  chrome.kill();
}
