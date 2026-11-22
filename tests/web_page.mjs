// 远程会话的网页端在真的浏览器里走一遍（无界面的 Chrome，照 iPhone 的屏幕大小、触摸）：打开页面、输入配对码、列出会话、接入、
// 敲字、按键条、输入框、菜单、断开再接入、会话结束、设置。tests/web_relay.py 的 PageTest 起好服务器和会话以后跑它；
// 也可以自己跑：node tests/web_page.mjs 网页端的地址 配对码 截图放哪个目录（DARK=1 是深色）。
// 结果是一段 JSON：ok、走到了哪几步、页面控制台里的报错（内容安全策略拦下的也在里面：应该一条都没有）。
// 这是 Chrome 的引擎，不是 iPhone 上的 Safari：键盘弹出、添加到主屏幕、手指滚动这些要在真的手机上看。
import { spawn } from 'node:child_process';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const [url, code, outDir] = process.argv.slice(2);
const profile = mkdtempSync(join(tmpdir(), 'cca-cdp-'));
const chrome = spawn(process.env.CCA_CHROME || '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', ['--headless=new', '--remote-debugging-port=0',
  `--user-data-dir=${profile}`, '--no-first-run', '--no-default-browser-check', '--disable-gpu', '--hide-scrollbars', '--no-proxy-server', 'about:blank'],
  { stdio: ['ignore', 'ignore', 'pipe'] });
// 出什么事都把 Chrome 停掉、它的临时资料目录删掉（这个脚本被叫停时也是：tests/web_relay.py 等不到会给整个进程组发信号）
let cleaned = false;
function cleanup() {
  if (cleaned) return;
  cleaned = true;
  try {
    chrome.kill('SIGKILL');
  } catch { /* 已经没了 */ }
  try {
    rmSync(profile, { recursive: true, force: true });
  } catch { /* 删不掉就算了 */ }
}
process.on('exit', cleanup);
for (const sig of ['SIGTERM', 'SIGINT', 'SIGHUP']) process.on(sig, () => process.exit(130));
process.on('uncaughtException', (e) => {
  console.log(JSON.stringify({ ok: false, error: 'exception: ' + String(e && e.message), steps: [], logs: [] }));
  process.exit(1);
});
process.on('unhandledRejection', (e) => {
  console.log(JSON.stringify({ ok: false, error: 'rejection: ' + String(e && e.message), steps: [], logs: [] }));
  process.exit(1);
});

const wsUrl = await new Promise((resolve, reject) => {
  let buf = '';
  chrome.stderr.on('data', (d) => {
    buf += d;
    const m = /DevTools listening on (ws:\/\/\S+)/.exec(buf);
    if (m) resolve(m[1]);
  });
  setTimeout(() => reject(new Error('Chrome 没起来：' + buf.slice(-500))), 20000);
});
const ws = new WebSocket(wsUrl);
await new Promise((r) => { ws.onopen = r; });
let seq = 0;
const waiting = new Map();
const logs = [];
ws.onmessage = (e) => {
  const m = JSON.parse(e.data);
  if (m.id && waiting.has(m.id)) {
    const { resolve, reject } = waiting.get(m.id);
    waiting.delete(m.id);
    m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result);
  } else if (m.method === 'Runtime.consoleAPICalled') logs.push('console.' + m.params.type + ': ' + m.params.args.map((a) => a.value ?? a.description).join(' '));
  else if (m.method === 'Runtime.exceptionThrown') logs.push('EXCEPTION: ' + (m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text));
  else if (m.method === 'Log.entryAdded') logs.push('log.' + m.params.entry.level + ': ' + m.params.entry.text + ' ' + (m.params.entry.url || ''));
};
const send = (method, params = {}, sessionId) => new Promise((resolve, reject) => {
  const id = ++seq;
  waiting.set(id, { resolve, reject });
  ws.send(JSON.stringify({ id, method, params, sessionId }));
});
const { targetId } = await send('Target.createTarget', { url: 'about:blank' });
const { sessionId } = await send('Target.attachToTarget', { targetId, flatten: true });
const page = (method, params) => send(method, params, sessionId);
await page('Page.enable');
await page('Runtime.enable');
await page('Log.enable');
const dark = process.env.DARK === '1';
await page('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 3, mobile: true });
await page('Emulation.setTouchEmulationEnabled', { enabled: true });
await page('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: dark ? 'dark' : 'light' }] });
await page('Emulation.setUserAgentOverride', { userAgent: 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1' });

const result = { steps: [] };
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function evaluate(expression) {
  const r = await page('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text);
  return r.result.value;
}
async function until(expression, secs = 20, what = expression) {
  const end = Date.now() + secs * 1000;
  for (;;) {
    if (await evaluate(expression)) return;
    if (Date.now() > end) throw new Error('没等到：' + what);
    await sleep(100);
  }
}
async function shot(name) {
  const { data } = await page('Page.captureScreenshot', { format: 'png' });
  writeFileSync(join(outDir, name + (dark ? '-dark' : '') + '.png'), Buffer.from(data, 'base64'));
}
async function tap(selector, text) {
  const pt = await evaluate(`(() => { const all = [...document.querySelectorAll(${JSON.stringify(selector)})];
    const el = ${text ? `all.find((e) => e.textContent.includes(${JSON.stringify(text)}))` : 'all[0]'};
    if (!el) return null; el.scrollIntoView({block: 'center', inline: 'center'}); const r = el.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; })()`);
  if (!pt) throw new Error('没有这个元素：' + selector + ' ' + (text || ''));
  result.lastTap = await evaluate(`(() => { const e = document.elementFromPoint(${pt[0]}, ${pt[1]}); return [${pt[0]}, ${pt[1]}, e && (e.tagName + '.' + e.className + ':' + e.textContent.slice(0, 12))]; })()`);
  await page('Input.dispatchTouchEvent', { type: 'touchStart', touchPoints: [{ x: pt[0], y: pt[1] }] });
  await page('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
  await sleep(150);
}
const typeText = (text) => page('Input.insertText', { text });

const step = (s) => { result.steps.push(s); };
try {
  await page('Page.navigate', { url });
  await until(`!!document.querySelector('.page')`, 20, '首页');
  await until(`!!document.querySelector('.primary')`, 20, '没配对时的引导');
  await sleep(300);
  await shot('1-welcome');
  step('welcome');
  await tap('.primary', '输入配对码');
  await until(`!!document.querySelector('input.code')`);
  await evaluate(`document.querySelector('input.code').focus()`);
  await typeText(code.toLowerCase().replace(/-/g, ''));
  await evaluate(`document.querySelector('input.code').dispatchEvent(new Event('input'))`);
  result.codeShown = await evaluate(`document.querySelector('input.code').value`);
  await shot('2-pair');
  await tap('.primary', '配对');
  await until(`!!document.querySelector('.session')`, 40, '配对后看到会话');
  await sleep(400);
  await shot('3-home');
  step('paired');
  result.home = await evaluate(`document.querySelector('.page').innerText`);
  await tap('.session');
  await until(`!!document.querySelector('.termview .xterm')`, 20, '终端');
  await until(`document.querySelector('.veil').hidden`, 30, '接入');
  await sleep(600);
  result.size = await evaluate(`(() => { const t = document.querySelector('.xterm-rows'); return [t.children.length, document.querySelector('.xterm-screen').clientWidth]; })()`);
  await evaluate(`document.querySelector('.xterm-helper-textarea').focus()`);
  await typeText('say 你好，手机');
  await page('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, text: '\r' });
  await page('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
  await until(`document.querySelector('.xterm-rows').innerText.includes('<你好，手机>')`, 15, '敲的字回来');
  step('typed');
  // 按键条：Ctrl 粘住再按 d（^D：假客户端读到头，会话结束）之前，先用条上的键发一条命令
  for (const ch of 'say bar') await typeText(ch);
  await tap('.keys button', '⏎');
  await until(`document.querySelector('.xterm-rows').innerText.includes('<bar>')`, 15, '按键条上的回车');
  step('keybar');
  await shot('4-terminal');
  await tap('.keys > button', '输入');
  await until(`!document.querySelector('.compose').hidden`);
  await evaluate(`document.querySelector('.compose textarea').focus()`);
  await typeText('say 从输入框来的一整句');
  await shot('5-compose');
  await tap('.compose button', '发送');
  await until(`document.querySelector('.xterm-rows').innerText.includes('<从输入框来的一整句>')`, 15, '输入框发送');
  step('compose');
  await tap('.keys > button', '输入');
  await tap('.top button:last-child');
  await until(`!!document.querySelector('.sheet')`);
  await sleep(300);
  await shot('6-menu');
  await tap('.sheet button', '+');
  await tap('.sheet button', '+');
  await tap('.shade');
  await sleep(400);
  result.sizeAfterFont = await evaluate(`document.querySelector('.xterm-rows').children.length`);
  step('menu');
  // 断开回到列表，再接入：回放里有刚才的输出
  await tap('.top button');
  await until(`!!document.querySelector('.session')`, 20, '回到列表');
  await tap('.session');
  await until(`!!document.querySelector('.termview .xterm') && document.querySelector('.veil').hidden`, 30, '再接入');
  await until(`document.querySelector('.xterm-rows').innerText.includes('<从输入框来的一整句>')`, 15, '回放');
  step('reattach');
  // 会话结束
  await evaluate(`document.querySelector('.xterm-helper-textarea').focus()`);
  await typeText('exit 3');
  await tap('.keys button', '⏎');
  await until(`!document.querySelector('.veil').hidden && document.querySelector('.veil').innerText.includes('会话结束了')`, 20, '会话结束的提示');
  result.ended = await evaluate(`document.querySelector('.veil').innerText`);
  await shot('7-ended');
  step('ended');
  await tap('.veil button', '回到列表');
  await until(`!!document.querySelector('.idle')`, 20, '没有会话时的说明');
  await sleep(300);
  await shot('8-idle');
  await tap('.bar button', '设置');
  await until(`!!document.querySelector('.rows')`);
  await shot('9-settings');
  step('settings');
  result.ok = true;
} catch (e) {
  result.ok = false;
  result.error = String(e.message || e);
  try {
    await shot('failed');
    result.dom = await evaluate(`document.body.innerText.slice(0, 600)`);
  } catch { /* 页面没了 */ }
}
result.logs = logs;
console.log(JSON.stringify(result, null, 1));
chrome.kill();
await Promise.race([new Promise((r) => chrome.once('exit', r)), sleep(5000)]);   // 等它退出再删资料目录（它还在写）
process.exit(result.ok ? 0 : 1);
