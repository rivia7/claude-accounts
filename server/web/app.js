// cca 远程会话的网页端：界面。协议和加密在 core.js；终端是 xterm.js（index.html 里先载入：window.Terminal、window.FitAddon）。
// 别人写的字（电脑的名字、会话的说明）一律当文字放进页面（append 字符串、textContent），不拼 HTML。
import { Attachment, Client, Failure } from './core.js';

const root = document.getElementById('app');
const MONO = 'ui-monospace, "SF Mono", Menlo, Consolas, monospace';

/** 建一个元素：props 里 on 开头的是事件，别的是属性；孩子是元素或者字（字不当 HTML） */
function h(tag, props, ...kids) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (k.startsWith('on')) el.addEventListener(k.slice(2), v);
    else if (v === true) el.setAttribute(k, '');
    else if (v !== false && v != null) el.setAttribute(k, v);
  }
  for (const kid of kids.flat()) if (kid != null && kid !== false) el.append(kid);
  return el;
}

// ---------------------------------------------------------------- 存东西的地方
// 密钥存在 IndexedDB 里（它存得下“不可导出”的密钥：页面只能拿来算，取不出原样的字节）；字号、设备的名字这些放 localStorage
const db = new Promise((resolve, reject) => {
  const r = indexedDB.open('cca-remote', 1);
  r.onupgradeneeded = () => r.result.createObjectStore('kv');
  r.onsuccess = () => resolve(r.result);
  r.onerror = () => reject(r.error);
});

function tx(mode, f) {
  return db.then((d) => new Promise((resolve, reject) => {
    const t = d.transaction('kv', mode);
    const q = f(t.objectStore('kv'));
    t.oncomplete = () => resolve(q.result);
    t.onerror = t.onabort = () => reject(t.error);
  }));
}

const store = {
  get: (k) => tx('readonly', (s) => s.get(k)),
  set: (k, v) => tx('readwrite', (s) => s.put(v, k)),
  del: (k) => tx('readwrite', (s) => s.delete(k)),
};

const prefs = {
  get(k, fallback) {
    try {
      return localStorage.getItem('cca.' + k) ?? fallback;
    } catch {
      return fallback;
    }
  },
  set(k, v) {
    try {
      localStorage.setItem('cca.' + k, String(v));
    } catch { /* 存不了就算了：下次还是默认的 */ }
  },
};

const ua = navigator.userAgent;
const isIOS = /iPhone|iPad|iPod/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1);
const standalone = navigator.standalone === true || window.matchMedia('(display-mode: standalone)').matches;
const deviceName = () => prefs.get('name', '') || (/iPad/.test(ua) || (/Macintosh/.test(ua) && navigator.maxTouchPoints > 1) ? 'iPad'
  : /iPhone/.test(ua) ? 'iPhone' : /Android/.test(ua) ? '手机' : '浏览器');

const client = new Client({ store, name: deviceName() });

// ---------------------------------------------------------------- 换一屏
let leave = null;

function show(view, onLeave) {
  const old = leave;
  leave = null;
  try {
    old?.();   // 上一屏的收尾出了错也照样换屏：不然之后哪儿都去不了
  } catch (e) {
    console.error(e);
  }
  leave = onLeave || null;
  root.replaceChildren(view);
  window.scrollTo(0, 0);
}

const TOOLS = { claude: 'Claude Code', codex: 'Codex', agy: 'Antigravity' };

/** 出了什么事，照实说、说怎么办 */
function say(e) {
  const code = e instanceof Failure ? e.code : '';
  return {
    unreachable: '连不上服务器。看看网络，再试一次。',
    rate: '问得太勤了，过几秒再试。',
    denied: '服务器不认这台设备了：电脑那边取消了配对的话，要重新配对。',
    unknown_device: '服务器不认这台设备了：电脑那边取消了配对的话，要重新配对。',
    code: '配对码是 16 个字母和数字，照电脑上显示的输入（大小写、横线随意）。',
    no_pairing: '这个配对码用不了：输错了、过了 10 分钟、已经用过，或者它不是给手机的（电脑上要运行 cca remote pair --web）。',
    no_answer: '没等到电脑那边确认。它那边还显示着配对码吗？重新来一次。',
    bad_peer: '对不上：服务器说的那台电脑和它自己说的不一样。没有配对。',
    mismatch: '两边算出来的不一样，没有配对。重新来一次。',
    edges: '配对的设备太多了：先在电脑上取消几个不用的。',
    clock: '这台设备的时间不对：调准了再试。',
    busy: '这个会话接入的已经够多了。',
  }[code] || '没成（' + (code || '出错了') + '）。再试一次。';
}

function ago(since) {
  const s = Date.now() / 1000 - since;
  if (!(since > 0) || s < 90) return '刚开始';
  if (s < 5400) return Math.round(s / 60) + ' 分钟前开始';
  if (s < 129600) return Math.round(s / 3600) + ' 小时前开始';
  return Math.round(s / 86400) + ' 天前开始';
}

// ---------------------------------------------------------------- 首页：配对过的电脑和它们上面的会话
function home() {
  let rows = null;
  let problem = '';
  let stopped = false;
  const list = h('div');
  const page = h('main', { class: 'page' },
    h('header', { class: 'bar' }, h('img', { src: 'logo.svg', alt: '' }), h('h1', null, '会话'),
      h('button', { class: 'plain', onclick: settings }, '设置')),
    list);

  function sessionRow(peer, s) {
    const state = !s.online ? '那台电脑这会儿没连着服务器' : s.access === 'view' ? '只能看，' + ago(s.since) : ago(s.since);
    return h('li', { class: 'branch' },
      h('button', { class: 'session', disabled: !s.online, onclick: () => terminal(peer, s) },
        h('strong', null, s.name || peer.name),
        h('span', null, (TOOLS[s.tool] || s.tool || '会话') + '，' + state),
        s.cwd ? h('span', { class: 'where' }, '‎' + s.cwd) : null));
  }

  function draw() {
    const out = [];
    if (problem) out.push(h('p', { class: 'problem' }, problem));
    if (rows === null) {
      if (!problem) out.push(h('p', { class: 'note' }, '正在看有哪些会话…'));
    } else if (!rows.length) {
      out.push(...welcome());
    } else {
      for (const r of rows) {
        out.push(h('section', { class: 'machine' },
          h('h2', null, r.peer.name, r.gone ? h('small', null, '那边取消了配对') : null),
          r.sessions.length ? h('ul', { class: 'branches' }, r.sessions.map((s) => sessionRow(r.peer, s)))
            : h('p', { class: 'idle' }, r.gone ? '在「设置」里把它删掉，要用的话重新配对。'
              : ['现在没有可以接入的会话。在那台电脑上先运行 ', h('code', null, 'cca remote on'), '，再在终端里启动会话。'])));
      }
      out.push(h('button', { class: 'plain', onclick: pairing }, '和另一台电脑配对'));
    }
    list.replaceChildren(...out);
  }

  function welcome() {
    return [
      isIOS && !standalone ? h('p', { class: 'problem' },
        '先把这一页添加到主屏幕：点 Safari 底下的分享按钮，选「添加到主屏幕」，之后从主屏幕打开再配对。主屏幕上的那一份有自己的数据，在这里配对的它用不上。') : null,
      h('p', null, '在手机上接着用电脑上的 Claude Code、Codex 会话。先让这台手机和电脑配对：'),
      h('ol', { class: 'steps' },
        h('li', null, '电脑上运行 ', h('code', null, 'cca remote on'), '：之后在终端里启动的会话才接得进来。'),
        h('li', null, '电脑上运行 ', h('code', null, 'cca remote pair --web'), '，它会显示一个配对码。'),
        h('li', null, '在这里输入那个配对码。')),
      h('button', { class: 'primary', onclick: pairing }, '输入配对码'),
    ];
  }

  async function refresh() {
    try {
      rows = await client.list();
      problem = '';
    } catch (e) {
      problem = say(e);
    }
    if (!stopped) draw();
  }

  const tick = () => document.hidden || refresh();
  const timer = setInterval(tick, 6000);   // 服务器给每台设备限了流（每 2 秒一个请求）：别问得更勤
  document.addEventListener('visibilitychange', tick);
  show(page, () => {
    stopped = true;
    clearInterval(timer);
    document.removeEventListener('visibilitychange', tick);
  });
  draw();
  refresh();
}

// ---------------------------------------------------------------- 配对
function pairing() {
  const code = h('input', {
    class: 'code', type: 'text', inputmode: 'text', autocapitalize: 'characters', autocomplete: 'off', autocorrect: 'off',
    spellcheck: 'false', placeholder: 'XXXX-XXXX-XXXX-XXXX', maxlength: '24', enterkeyhint: 'go',
  });
  const name = h('input', { type: 'text', value: deviceName(), maxlength: '40', autocomplete: 'off' });
  const status = h('p', { class: 'note' }, '配对码在电脑上：运行 cca remote pair --web（或者在 cca App 的「远程会话」里点「和手机配对…」）。10 分钟内有效，只能用一次。');
  const go = h('button', { class: 'primary' }, '配对');

  // 边输边分成四个一组（只是好认；大小写、横线怎么输都行）
  code.addEventListener('input', () => {
    const raw = code.value.toUpperCase().replace(/[^0-9A-Z]/g, '').slice(0, 16);
    code.value = raw.replace(/(.{4})(?=.)/g, '$1-');
  });

  let here = true;   // 还在这一屏（配到一半点了「取消」：配成了也不把人拉回首页）
  async function run() {
    if (go.disabled) return;
    go.disabled = true;
    status.className = 'note';
    const mine = name.value.trim().slice(0, 40);
    if (mine) prefs.set('name', mine);
    client.name = mine || deviceName();
    try {
      const steps = { deriving: '正在算密钥…', asking: '正在等电脑那边…', confirming: '正在确认…' };
      await client.pair(code.value, (s) => { status.textContent = steps[s] || ''; });
      if (here) home();
    } catch (e) {
      status.className = 'problem';
      status.textContent = say(e);
      go.disabled = false;
    }
  }

  go.addEventListener('click', run);
  code.addEventListener('keydown', (e) => e.key === 'Enter' && run());
  show(h('main', { class: 'page' },
    h('header', { class: 'bar' }, h('h1', null, '配对'), h('button', { class: 'plain', onclick: home }, '取消')),
    h('label', { class: 'field' }, h('span', null, '电脑上显示的配对码'), code),
    h('label', { class: 'field' }, h('span', null, '这台设备叫什么（电脑上看到的名字）'), name),
    go, status), () => { here = false; });
  code.focus();
}

// ---------------------------------------------------------------- 设置
async function settings() {
  const peers = await client.peers().catch(() => []);
  const id = await client.identity().catch(() => null);
  const name = h('input', { type: 'text', value: deviceName(), maxlength: '40', autocomplete: 'off' });
  name.addEventListener('change', () => {
    const v = name.value.trim().slice(0, 40);
    if (v) prefs.set('name', v);
    client.name = v || deviceName();
  });

  async function drop(p, li) {
    if (!window.confirm('取消和「' + p.name + '」的配对？之后这台设备接不进它上面的会话，要用得重新配对。')) return;
    const told = await client.unpair(p.pid);
    li.replaceChildren(h('div', null, '已经取消',
      told ? null : h('small', null, '没告诉到服务器：电脑上运行 cca remote unpair 把它也删掉。')));
  }

  show(h('main', { class: 'page' },
    h('header', { class: 'bar' }, h('h1', null, '设置'), h('button', { class: 'plain', onclick: home }, '完成')),
    h('label', { class: 'field' }, h('span', null, '这台设备叫什么（下次配对时告诉电脑）'), name),
    h('h3', null, '配对过的电脑'),
    peers.length ? h('ul', { class: 'rows' }, peers.map((p) => {
      const li = h('li', null, h('div', null, p.name, h('small', null, new Date(p.at).toLocaleDateString('zh-CN') + ' 配对')));
      li.append(h('button', { class: 'plain danger', onclick: () => drop(p, li) }, '取消配对'));
      return li;
    })) : h('p', { class: 'note' }, '还没有。'),
    h('button', { class: 'plain', onclick: pairing }, '和一台电脑配对'),
    h('h3', null, '这一页靠什么保密'),
    h('p', { class: 'note' }, '终端里的内容在这台设备和电脑之间加密，密钥是配对时两边各自算出来的，服务器只转密文、看不到内容。'
      + '不过这一页本身是服务器给的：管着这台服务器的人能换掉页面的代码。只用你信得过的服务器。'),
    h('p', { class: 'note' }, '密钥存在这个浏览器里。清掉网站数据、删掉主屏幕上的图标，就要重新配对。'
      + (id ? '这台设备的指纹：' + id.fp.slice(7, 19) + '…' : ''))));
}

// ---------------------------------------------------------------- 终端
/** 按键条上的一个键。手指抬起来才算按（按下以后划动的是在滚动这一排：不算）；按下时不让焦点离开终端（不然键盘会收起来）。
 *  repeat：按住不动一会儿就连发（方向键） */
function press(el, fn, repeat) {
  let timer = null;
  let start = null;
  let held = false;
  const clear = () => {
    clearTimeout(timer);
    clearInterval(timer);
    timer = null;
    el.classList.remove('down');
  };
  const cancel = () => {
    start = null;
    clear();
  };
  el.addEventListener('pointerdown', (e) => {
    e.preventDefault();
    clear();   // 上一根手指还按着：它的连发先停掉
    start = { id: e.pointerId, x: e.clientX, y: e.clientY };
    held = false;
    el.classList.add('down');
    if (repeat) {
      timer = setTimeout(() => {
        held = true;
        fn();
        timer = setInterval(fn, 70);
      }, 380);
    }
  });
  el.addEventListener('pointermove', (e) => {
    if (start && e.pointerId === start.id && Math.hypot(e.clientX - start.x, e.clientY - start.y) > 10) cancel();
  });
  el.addEventListener('pointerup', (e) => {
    const mine = start && e.pointerId === start.id;
    cancel();
    if (mine && !held) fn();
  });
  for (const ev of ['pointercancel', 'pointerleave']) el.addEventListener(ev, cancel);   // 浏览器接手去滚动了
  el.addEventListener('click', (e) => e.detail === 0 && fn());   // 键盘、读屏的“点按”
  return el;
}

const CLOSED = {
  gone: '这个会话不在了：它结束了，或者那台电脑不再让这台设备接入。',
  unpaired: '配对没有了：电脑那边取消了。要用得重新配对。',
  gave_up: '一直没接上。那台电脑可能睡了、断网了。',
  refused: '那台电脑不认这个配对：它那边取消了，或者不让这台设备接入了。',
  version: '两边的版本对不上：更新电脑上的 cca，再刷新这一页。',
  auth: '对面证明不了它有配对的密钥，没有接入（什么都没发给它）。重新配对试试。',
  busy: '这个会话接入的已经够多了。',
};

function terminal(peer, session) {
  const readOnly = session.access === 'view';
  const where = session.cwd ? session.cwd.replace(/\/+$/, '').split('/').pop() : '';
  const screen = h('div', { class: 'screen' });
  const veil = h('div', { class: 'veil' }, '正在接入…');
  const keys = h('div', { class: 'scroll' });
  const composeBox = h('textarea', { rows: '1', placeholder: '在这里打字（能用输入法、听写），发送后回车', enterkeyhint: 'send', autocapitalize: 'off' });
  const compose = h('div', { class: 'compose', hidden: true }, composeBox);
  const view = h('div', { class: 'termview' },
    h('div', { class: 'top' },
      h('button', { 'aria-label': '断开，回到列表', onclick: () => home() }, '‹'),
      h('div', null, session.name || peer.name, h('small', null, readOnly ? '只能看' : where)),
      h('button', { 'aria-label': '菜单', onclick: () => menu() }, '⋯')),
    screen, compose,
    readOnly ? null : h('div', { class: 'keys' }, keys));

  // 收尾里用到的先声明：下面哪一步出了错（比如终端的脚本没载入），离开这一屏时也不会再出一次错
  let gone = false;
  let att = null;
  let term = null;
  let lock = null;
  const vv = window.visualViewport;
  show(view, () => {
    gone = true;
    document.body.classList.remove('in-terminal');
    att?.detach();
    term?.dispose();
    vv?.removeEventListener('resize', place);
    vv?.removeEventListener('scroll', place);
    window.removeEventListener('resize', place);
    document.removeEventListener('visibilitychange', seen);
    lock?.release?.().catch(() => {});
  });
  document.body.classList.add('in-terminal');   // 换屏之后才加：上一屏也是终端（「再接一次」）时，它的收尾会把这个类名去掉
  screen.append(veil);

  term = new window.Terminal({
    fontFamily: MONO, fontSize: Number(prefs.get('font', 13)) || 13, scrollback: 4000, cursorBlink: false, macOptionIsMeta: true,
    theme: { background: '#000000', foreground: '#e8e9ec', cursor: '#e8e9ec', selectionBackground: '#3b7bff88' },
  });
  const fit = new window.FitAddon.FitAddon();
  term.loadAddon(fit);
  term.open(screen);

  // 键盘出来时页面看得见的那一块变矮（visualViewport）：整屏跟着它，按键条贴在键盘上面，终端重新算行数
  let fitting = 0;
  function refit() {
    cancelAnimationFrame(fitting);
    fitting = requestAnimationFrame(() => {
      if (gone) return;
      try {
        fit.fit();
      } catch { /* 还没排好版 */ }
    });
  }
  function place() {
    const height = vv ? vv.height : window.innerHeight;
    view.style.setProperty('--vvh', height + 'px');
    view.style.setProperty('--vvt', (vv ? vv.offsetTop : 0) + 'px');
    if (window.innerHeight - height > 120) view.style.setProperty('--kb', '0px');   // 键盘在：底下不用再让出主屏幕横条的位置
    else view.style.removeProperty('--kb');
    refit();
  }
  vv?.addEventListener('resize', place);
  vv?.addEventListener('scroll', place);
  window.addEventListener('resize', place);
  place();
  try {
    fit.fit();
  } catch { /* 同上 */ }

  // 接入。断了它自己重新接上；重新接上以后那边会重放模式和最近的输出，所以先把终端清回刚打开的样子
  let opened = false;
  let exit = null;
  function cover(text, over, buttons) {
    veil.className = over ? 'veil over' : 'veil';
    veil.hidden = false;
    veil.replaceChildren(h('div', { class: over ? null : 'pulse' }, text), ...(buttons || []).filter(Boolean));
  }
  att = new Attachment(client, peer, session.sid, {
    cols: term.cols, rows: term.rows, readOnly,
    on: {
      output: (b) => term.write(b),
      exit: (code) => { exit = code; },
      state(state, why) {
        if (gone) return;
        if (state === 'open') {
          if (opened) term.write('\x18\x1bc');
          opened = true;
          veil.hidden = true;
        } else if (state === 'retrying') {
          cover(opened ? '连接断了，正在重新接上…现在敲的键送不过去。' : '正在接入…那台电脑还没应。');
        } else if (state === 'closed' && why !== 'detached') {
          const text = why === 'ended' ? '会话结束了' + (exit ? '（退出码 ' + exit + '）。' : '。') : CLOSED[why] || '没接上。';
          const again = ['gave_up', 'busy'].includes(why) || !CLOSED[why] && why !== 'ended';
          cover(text, true, [
            again ? h('button', { class: 'primary', onclick: () => terminal(peer, session) }, '再接一次') : null,
            h('button', { class: again ? 'plain' : 'primary', onclick: home }, '回到列表')]);
          term.blur();
        }
      },
    },
  });

  // 敲的键。Ctrl 是“粘”的：点一下，下一个键当成 Ctrl 加它
  let ctrl = null;
  let armed = false;
  function arm(on) {
    armed = on;
    ctrl?.classList.toggle('armed', on);
  }
  term.onData((d) => {
    if (armed && d.length === 1) {
      const c = d.toUpperCase().charCodeAt(0);
      if (c >= 0x40 && c <= 0x5f) d = String.fromCharCode(c & 0x1f);
      else if (d === ' ') d = '\0';
      arm(false);
    }
    att.input(d);
  });
  term.onBinary((d) => att.input(Uint8Array.from(d, (ch) => ch.charCodeAt(0) & 0xff)));
  term.onResize(({ cols, rows }) => att.resize(cols, rows));

  if (!readOnly) {
    const arrow = (letter) => () => att.input((term.modes.applicationCursorKeysMode ? '\x1bO' : '\x1b[') + letter);
    const key = (label, what, repeat, title) => press(h('button', { 'aria-label': title || label }, label),
      typeof what === 'string' ? () => att.input(what) : what, repeat);
    ctrl = key('Ctrl', () => arm(!armed));
    keys.append(key('Esc', '\x1b'), key('Tab', '\t'), key('⇧Tab', '\x1b[Z', false, 'Shift Tab'), ctrl,
      key('↑', arrow('A'), true, '上'), key('↓', arrow('B'), true, '下'), key('←', arrow('D'), true, '左'), key('→', arrow('C'), true, '右'),
      key('⏎', '\r', false, '回车'), key('^C', '\x03', false, 'Ctrl C'), key('^D', '\x04', false, 'Ctrl D'),
      key('/', '/'), key('@', '@'), key('~', '~'), key('|', '|'), key('-', '-'));
    // 输入框：长一点的话、中文、听写在这里打，发送时整段送过去再回车
    const toggle = h('button', { 'aria-label': '输入框' }, '输入');
    toggle.addEventListener('click', () => {
      compose.hidden = !compose.hidden;
      place();
      if (compose.hidden) term.focus();
      else composeBox.focus();
    });
    const sendBox = () => {
      if (att.state !== 'open') return;   // 断着：字留在输入框里，接上了再发（不然一整段话就没了）
      const text = composeBox.value;
      composeBox.value = '';
      if (text) term.paste(text);
      setTimeout(() => att.input('\r'), text ? 60 : 0);   // 先让那边收完这一段，再回车
    };
    compose.append(press(h('button', null, '发送'), sendBox));
    composeBox.addEventListener('keydown', (e) => {
      // 输入法选字时的回车不算（Safari 里那一下的 isComposing 已经是 false，要看 keyCode 是不是 229）
      if (e.key === 'Enter' && !e.shiftKey && !e.isComposing && e.keyCode !== 229) {
        e.preventDefault();
        sendBox();
      }
    });
    keys.parentElement.append(toggle);
  }

  // 菜单
  function menu() {
    const close = () => {
      sheet.remove();
      shade.remove();
    };
    const shade = h('div', { class: 'shade', onclick: close });
    const note = h('p', { class: 'note', hidden: true });
    const font = (d) => () => {
      const n = Math.max(9, Math.min(22, term.options.fontSize + d));
      term.options.fontSize = n;
      prefs.set('font', n);
      refit();
    };
    const item = (label, fn) => h('button', { onclick: fn }, label);
    const sheet = h('div', { class: 'sheet' },
      h('div', { class: 'size' }, h('span', null, '字的大小'), item('−', font(-1)), item('+', font(1))),
      readOnly ? null : item('粘贴', async () => {
        try {
          const text = await navigator.clipboard.readText();
          close();
          if (text) term.paste(text);
        } catch {
          note.hidden = false;
          note.textContent = '浏览器没让读剪贴板：点「输入」，在输入框里长按粘贴。';
        }
      }),
      item('复制屏幕上的字', async () => {
        const buf = term.buffer.active;
        const lines = [];
        for (let y = buf.viewportY; y < buf.viewportY + term.rows; y++) lines.push(buf.getLine(y)?.translateToString(true) ?? '');
        try {
          await navigator.clipboard.writeText(term.getSelection() || lines.join('\n').replace(/\n+$/, ''));
          close();
        } catch {
          note.hidden = false;
          note.textContent = '浏览器没让写剪贴板。';
        }
      }),
      item('断开（会话接着运行）', home), note);
    view.append(shade, sheet);
  }

  // 回到前台：断着的话马上再试，不等退避的那几秒；屏幕别在看着的时候自己灭掉
  async function awake() {
    try {
      const got = await navigator.wakeLock?.request('screen');
      if (gone) got?.release?.().catch(() => {});   // 还没拿到人就走了：还回去
      else lock = got || null;
    } catch { /* 不给就算了 */ }
  }
  function seen() {
    if (document.hidden) return;
    att.nudge();
    awake();
  }
  document.addEventListener('visibilitychange', seen);
  awake();
  if (!readOnly && !isIOS) term.focus();   // 手机上不自己弹键盘：先看一眼屏幕，点一下终端再出来
}

// ---------------------------------------------------------------- 打开
async function boot() {
  if (!window.isSecureContext || !crypto.subtle) {
    return root.replaceChildren(h('p', { class: 'boot' }, '这一页要经 HTTPS 打开才能用。'));
  }
  try {
    await client.identity();
  } catch (e) {
    const old = e && (e.name === 'NotSupportedError' || e.name === 'DataError');
    return root.replaceChildren(h('p', { class: 'boot' }, old
      ? '这个浏览器太旧了：要 iOS 17 以上的 Safari（或者同样新的别的浏览器）。'
      : '这个浏览器存不了数据（开着无痕浏览？），用不了。'));
  }
  navigator.storage?.persist?.().catch(() => {});
  return home();
}

boot();
