// 网页端的协议层（server/web/core.js）在 Node 里跑：tests/web_relay.py 起它，一行一条 JSON 命令从 stdin 进来、一行一条结果从 stdout 出去。
// 对面是真的服务器、真的 cca（或者 tests/spec_wire.py）；这里不判断对错，只照命令做、把看到的报回去。要 Node 22 以上（自带 WebSocket）。
import { createInterface } from 'node:readline';
import * as core from '../server/web/core.js';

const mem = new Map();
const store = { get: async (k) => mem.get(k), set: async (k, v) => void mem.set(k, v), del: async (k) => void mem.delete(k) };
let client = null;
let att = null;
let out = [];
const events = [];
const b = (s) => core.unb64(s);

function seen() {
  return Buffer.concat(out.map((x) => Buffer.from(x))).toString('latin1');
}

async function until(test, secs) {
  const end = Date.now() + secs * 1000;
  while (!test()) {
    if (Date.now() > end) return false;
    await new Promise((r) => setTimeout(r, 30));
  }
  return true;
}

const cmds = {
  // 照给的输入算一遍，结果交回去和 spec_wire.py 算的比
  async vectors(a) {
    const code = core.codeBytes(a.code);
    const k = await core.pairKeys(code);
    const na = b(a.na);
    const nb = b(a.nb);
    const { p, pid } = core.pairSecret(k.kc, na, nb, a.fpa, a.fpb);
    const opened = core.pairOpen(k.keys, k.rid, 'a', a.msg_a);
    const kd = new core.Hmac(core.hmac(p, core.utf8('cca-remote/v1/dir')));
    const dkeys = { enc: new core.Hmac(kd.mac(core.utf8('enc'))), mac: new core.Hmac(kd.mac(core.utf8('mac'))) };
    return {
      code_text: core.codeText(code), rid: k.rid, p: core.hex(p), pid,
      sha: core.hex(core.sha256(b(a.blob))), mac: core.hex(core.hmac(b(a.key), b(a.blob))),
      opened: opened && { fp: opened.fp, n: core.hex(opened.n), name: opened.name },
      msg_b: core.pairMsg(k.keys, k.rid, 'b', a.fpb, nb, a.name_b),
      dir: core.dirOpen(dkeys, pid, a.sid, a.dir), dir_bad: core.dirOpen(dkeys, pid, a.sid.replace(/^./, (c) => (c === '0' ? '1' : '0')), a.dir),
      bad_codes: a.bad_codes.map((c) => core.codeBytes(c) === null),
    };
  },
  // 通道：这一头是接入方，对面的三步握手和记录由 spec_wire.py 出
  async channel(a) {
    const ka = core.hmac(b(a.p), core.utf8('cca-remote/v1/auth'));
    const ch = (cmds.ch = new core.Channel(ka, a.pid, a.sid, a.fph, a.fpv));
    return { hello: core.b64(ch.hello()) };
  },
  async confirm(a) {
    const c = cmds.ch.confirm(b(a.reply));
    return { confirm: c && core.b64(c) };
  },
  async seal(a) {
    return { records: a.data.map((d) => core.b64(cmds.ch.seal(b(d)))) };
  },
  async open(a) {
    let buf = b(a.bytes);
    const got = [];
    try {
      for (;;) {
        const r = cmds.ch.open(buf);
        if (!r) break;
        got.push(core.b64(r.data));
        buf = buf.subarray(r.used);
      }
    } catch (e) {
      return { got, error: e.message };
    }
    return { got, left: buf.length };
  },
  async sign(a) {
    const saved = await core.Identity.create();
    const id = new core.Identity(saved.privateKey, saved.pub);
    return { pub: id.pub, fp: id.fp, sig: await id.sign(b(a.data)) };
  },
  async start(a) {
    client = new core.Client({ base: a.base, store, name: a.name });
    const id = await client.identity();
    return { fp: id.fp, pub: id.pub };
  },
  async pair(a) {
    const steps = [];
    const peer = await client.pair(a.code, (s) => steps.push(s));
    return { pid: peer.pid, fp: peer.fp, name: peer.name, steps, extractable: peer.key.extractable };
  },
  async unpair(a) {
    return { told: await client.unpair(a.pid) };
  },
  async list() {
    return { rows: (await client.list()).map((r) => ({ pid: r.peer.pid, name: r.peer.name, gone: r.gone, sessions: r.sessions })) };
  },
  async attach(a) {
    out = [];
    events.length = 0;
    const peer = (await client.peers()).find((p) => p.pid === a.pid);
    att = new core.Attachment(client, peer, a.sid, {
      cols: a.cols, rows: a.rows, readOnly: !!a.read_only,
      on: { output: (x) => out.push(x.slice()), state: (s, why) => events.push([s, why]), exit: (code) => events.push(['exit', code]) },
    });
    return {};
  },
  // 等到收到的输出里有这段字（latin1 的写法），或者到了某个状态
  async wait(a) {
    const ok = await until(() => (a.text ? seen().includes(a.text) : events.some((e) => e[0] === a.state && (!a.why || e[1] === a.why))), a.secs || 20);
    return { ok, tail: seen().slice(-400), events: events.slice(), count: a.count ? seen().split(a.count).length - 1 : 0 };
  },
  async input(a) {
    att.input(a.text !== undefined ? a.text : b(a.data));
    return {};
  },
  async resize(a) {
    att.resize(a.cols, a.rows);
    return {};
  },
  async detach() {
    att.detach();
    return { events: events.slice() };
  },
  async clear() {
    out = [];
    return {};
  },
  // 把正接着的那条连接从底下掐断（网络断了）：看它自己接不接得回来
  async cut() {
    att.link.ws.close();
    return {};
  },
};

for await (const line of createInterface({ input: process.stdin })) {
  let reply;
  try {
    const a = JSON.parse(line);
    reply = { ok: true, ...(await cmds[a.cmd](a)) };
  } catch (e) {
    reply = { ok: false, error: e.code || 'exception', message: String(e && e.message), status: e.status || 0 };
  }
  process.stdout.write(JSON.stringify(reply) + '\n');
}
process.exit(0);
