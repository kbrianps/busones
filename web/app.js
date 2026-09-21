/* busones: Rio buses in the interface riders already know.
 *
 * The layout follows Lá vem o ônibus on purpose: a map filling the screen,
 * the search pill on top, the lines you follow as coloured circles on the
 * left, and a sheet with the stop's arrival times. What changes is only what
 * its own users complain about: the direction is always written out, markers
 * grow with the zoom, a stop shows every line that serves it, lines are
 * removed one at a time, and times are ranges built from how fast the line's
 * buses are moving right now instead of a single minute that turns out wrong.
 */

const API = '/run/api/v1';
/* Address search and reverse geocoding: answered by the backend, which caches
   Nominatim (in production Caddy proxies these paths to it). */
const GEO = '/api/v1';
const EST = '/dist';
const Z = 13;
const RAIO_PERTO = 600;
const RECARGA_MS = 12000;
const CENTRO = [-22.9068, -43.1729];

/* The palette from onibus-rj, the first version of this app, so the lines
   you follow keep the colours they had there. */
const CORES = [
  'hsl(199, 89%, 48%)', 'hsl(28, 89%, 48%)', 'hsl(140, 70%, 38%)',
  'hsl(270, 70%, 55%)', 'hsl(330, 80%, 50%)', 'hsl(170, 75%, 38%)',
  'hsl(245, 75%, 55%)', 'hsl(45, 85%, 47%)', 'hsl(305, 70%, 48%)',
];

/* The badge used in lists: always the same width, so destinations line up in
   one column whatever the code ("3" or "LECD148"); the type shrinks instead. */
function selo(l, extra = '') {
  const s = el('span', 'selo' + (extra ? ' ' + extra : ''), l);
  s.dataset.tam = String(Math.min(7, Math.max(4, l.length)));
  return s;
}

/* The line badge from onibus-rj: a rounded square with the number, the type
   shrinking as the code gets longer ("474" to "LECD133"). */
function numero(l, tag = 'div') {
  const n = el(tag, 'numero', l);
  n.style.setProperty('--cor', corDaLinha(l));
  n.dataset.tam = l.length <= 4 ? 'p' : l.length <= 6 ? 'm' : 'g';
  return n;
}

/* ---------- helpers ---------- */
const $ = s => document.querySelector(s);
function el(tag, cls, txt) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (txt != null) e.textContent = txt;
  return e;
}
function icone(nome) {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  const u = document.createElementNS('http://www.w3.org/2000/svg', 'use');
  u.setAttribute('href', '#i-' + nome);
  s.append(u);
  return s;
}
const metros = m => m >= 1000 ? (m / 1000).toFixed(1).replace('.', ',') + ' km' : Math.round(m) + ' m';
const aPe = m => `${Math.max(1, Math.round(m / 75))} min a pé`;
/* The incumbent writes data age as "52 seg atrás" and "1:18 atrás". */
function atras(seg) {
  seg = Math.max(0, Math.round(seg));
  if (seg < 60) return `${seg} seg atrás`;
  return `${Math.floor(seg / 60)}:${String(seg % 60).padStart(2, '0')} atrás`;
}
function distM(a, b) {
  const p = Math.PI / 180;
  const dla = (b[0] - a[0]) * p, dlo = (b[1] - a[1]) * p;
  const x = Math.sin(dla / 2) ** 2 + Math.cos(a[0] * p) * Math.cos(b[0] * p) * Math.sin(dlo / 2) ** 2;
  return 12742000 * Math.asin(Math.sqrt(x));
}
function tile(lat, lon, z = Z) {
  const n = 2 ** z, r = lat * Math.PI / 180;
  return [Math.floor((lon + 180) / 360 * n),
          Math.floor((1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2 * n)];
}
function decodifica(str) {
  const out = [];
  let i = 0, lat = 0, lon = 0;
  while (i < str.length) {
    for (const k of [0, 1]) {
      let r = 0, s = 0, b;
      do { b = str.charCodeAt(i++) - 63; r |= (b & 31) << s; s += 5; } while (b >= 32);
      const d = r & 1 ? ~(r >> 1) : r >> 1;
      if (k === 0) lat += d; else lon += d;
    }
    out.push([lat / 1e5, lon / 1e5]);
  }
  return out;
}

/* Relative times are only true if the phone's clock is; the server's Date
   header on every response corrects it. */
let desvio = 0;
const agoraS = () => Date.now() / 1000 + desvio;
async function pega(url) {
  const r = await fetch(url, { cache: 'no-cache' });
  if (!r.ok) throw new Error(`${r.status} ${url}`);
  const d = Date.parse(r.headers.get('date') || '');
  if (isFinite(d)) {
    const delta = d / 1000 - Date.now() / 1000;
    if (Math.abs(delta) > 2) desvio = delta;
  }
  return r.json();
}

/* ---------- state ---------- */
const est = {
  eu: [...CENTRO],
  temLocal: false,
  minhas: [],
  sentido: {},
  catalogo: [],
  bundles: new Map(),
  frotas: new Map(),
  chegadas: [],
  paradas: new Map(),
  celulasParadas: new Set(),
  folha: null,
  aba: 'vivo',
  veiculosFolha: [],
  busca: '',
  buscando: false,
  balao: null,
  ajustes: { escuro: false, rota: true, paradas: true, semSinal: true },
  local: { modo: null, nome: '' },
  /* Lines in focus. Empty means every line you follow is on the map. */
  focadas: new Set(),
  freq: null,
  escolhendoLocal: false,
  todasParadas: null,
  buscadoEm: 0,
  erro: '',
};

function salva() {
  try {
    localStorage.setItem('busones:linhas', JSON.stringify(est.minhas));
    localStorage.setItem('busones:sentido', JSON.stringify(est.sentido));
    localStorage.setItem('busones:ajustes', JSON.stringify(est.ajustes));
  } catch {}
}
function carrega() {
  try {
    const l = JSON.parse(localStorage.getItem('busones:linhas') || '[]');
    if (Array.isArray(l)) est.minhas = l.filter(x => typeof x === 'string').slice(0, 8);
    est.sentido = JSON.parse(localStorage.getItem('busones:sentido') || '{}') || {};
    const a = JSON.parse(localStorage.getItem('busones:ajustes') || 'null');
    if (a) Object.assign(est.ajustes, a);
    else est.ajustes.escuro = matchMedia('(prefers-color-scheme: dark)').matches;
  } catch {}
}

function corDaLinha(l) {
  const i = est.minhas.indexOf(l);
  return i >= 0 ? CORES[i % CORES.length] : '#303f9f';
}

/* ---------- data ---------- */
async function bundle(l) {
  if (est.bundles.has(l)) return est.bundles.get(l);
  const b = await pega(`${EST}/lines/${encodeURIComponent(l)}.json`).catch(() => null);
  if (b) {
    for (const d of b.dirs) {
      d.pts = decodifica(d.poly);
      for (const s of d.stops) {
        if (!est.paradas.has(s[0])) est.paradas.set(s[0], { id: s[0], lat: s[1], lon: s[2], nome: s[3], sub: s[5] || '', linhas: null });
      }
    }
  }
  est.bundles.set(l, b);
  return b;
}

async function paradasEm(lat, lon) {
  const [x, y] = tile(lat, lon);
  const pedidos = [];
  for (let dx = -1; dx <= 1; dx++) for (let dy = -1; dy <= 1; dy++) {
    const k = `${x + dx}/${y + dy}`;
    if (est.celulasParadas.has(k)) continue;
    est.celulasParadas.add(k);
    pedidos.push(pega(`${EST}/cells/${Z}/${k}/stops.json`).catch(() => []));
  }
  for (const l of await Promise.all(pedidos)) {
    for (const s of l) est.paradas.set(s[0], { id: s[0], lat: s[1], lon: s[2], nome: s[3], sub: s[4] || '', linhas: s[5] });
  }
}

/* For one direction of a line, the stop nearest to you. */
function paradaPerto(dir) {
  let m = null;
  for (const s of dir.stops) {
    const d = distM(est.eu, [s[1], s[2]]);
    if (!m || d < m.dist) m = { id: s[0], lat: s[1], lon: s[2], nome: s[3], sub: s[5] || '', dist: d };
  }
  return m;
}

/* The direction a followed line is shown in: the one you last picked, or the
   one whose stop is closest to you. Lá vem hides this behind "mudar sentido";
   here it is always written on screen. */
function dirAtual(l) {
  const b = est.bundles.get(l);
  if (!b || !b.dirs.length) return null;
  const pedido = est.sentido[l];
  const d = b.dirs.find(x => x.headsign === pedido);
  if (d) return d;
  return [...b.dirs].sort((a, c) => (paradaPerto(a)?.dist ?? 1e9) - (paradaPerto(c)?.dist ?? 1e9))[0];
}

async function atualiza() {
  try {
    const cels = new Set();
    const add = (la, lo) => { const [x, y] = tile(la, lo); cels.add(`${x}/${y}`); };
    add(est.eu[0], est.eu[1]);
    for (const l of est.minhas) {
      const b = await bundle(l);
      if (b) for (const d of b.dirs) { const p = paradaPerto(d); if (p) add(p.lat, p.lon); }
    }
    if (est.folha) add(est.folha.lat, est.folha.lon);

    const pedidosFolha = [];
    if (est.folha) {
      const [x, y] = tile(est.folha.lat, est.folha.lon);
      for (let dx = -1; dx <= 1; dx++) for (let dy = -1; dy <= 1; dy++)
        pedidosFolha.push(pega(`${API}/cells/${Z}/${x + dx}/${y + dy}/vehicles.json`).catch(() => []));
    }
    const [cheg, frotas, vf] = await Promise.all([
      Promise.all([...cels].map(k => pega(`${API}/cells/${Z}/${k}/arrivals.json`).catch(() => []))),
      Promise.all(est.minhas.map(l => pega(`${API}/lines/${encodeURIComponent(l)}.json`).catch(() => []))),
      Promise.all(pedidosFolha),
    ]);
    est.chegadas = cheg.flat();
    est.minhas.forEach((l, i) => est.frotas.set(l, frotas[i].map(v => ({ ...v, line: l }))));
    est.veiculosFolha = vf.flat();
    est.buscadoEm = Date.now();
    est.erro = '';
  } catch {
    est.erro = 'Sem conexão';
  }
  desenha();
}

/* ---------- time ---------- */
function faixa(a) {
  const passou = agoraS() - a[6];
  const lo = Math.max(0, a[8] - passou) / 60;
  const hi = Math.max(0, a[9] - passou) / 60;
  if (a[5] <= 150 || hi <= 1.2) return { chegando: true };
  const l = Math.max(1, Math.floor(lo));
  return { l, h: Math.max(l + 1, Math.ceil(hi)) };
}
const textoFaixa = f => f.chegando ? 'chegando' : `em ${f.l} a ${f.h} min`;

function chegadasEm(stopId, linha, headsign) {
  return est.chegadas
    .filter(a => a[0] === stopId && (!linha || a[1] === linha) && (!headsign || a[2] === headsign))
    .sort((x, y) => (x[8] + x[9]) / 2 - (x[6] - agoraS()) - ((y[8] + y[9]) / 2 - (y[6] - agoraS())));
}

/* Scheduled headway for this hour, from the published frequencies. */
function intervaloAgora(dir) {
  if (!dir || !dir.freq) return null;
  const d = new Date();
  const svc = d.getDay() === 0 ? 2 : d.getDay() === 6 ? 1 : 0;
  const h = d.getHours();
  const f = dir.freq.find(x => x[0] === svc && h >= x[1] && h < x[2]);
  return f ? f[3] : null;
}

/* ---------- left rail ---------- */
function desenhaTrilho() {
  const t = $('#trilho');
  t.textContent = '';
  // Below every line, a last badge that clears them all. The rail stacks
  // upward, so the first child is the one at the bottom.
  if (est.minhas.length) {
    const x = el('button', 'numero limpar', '×');
    x.setAttribute('aria-label', 'Remover todas as linhas');
    x.title = 'Remover todas as linhas';
    x.onclick = e => { e.stopPropagation(); removeTodas(); };
    t.append(x);
  }
  for (const l of est.minhas) {
    const c = numero(l, 'button');
    if (est.focadas.has(l)) c.classList.add('ativo');
    else if (est.focadas.size) c.classList.add('apagado');
    c.setAttribute('aria-label', `Linha ${l}, opções`);
    c.setAttribute('aria-haspopup', 'menu');
    c.onclick = e => { e.stopPropagation(); abreMenuLinha(l, c); };
    t.append(c);
  }
}

/* ---------- line menu, as in onibus-rj ----------
 * Tapping a line's badge opens a small menu beside it: show only this line,
 * pick the direction you ride, see the times at your stop, or remove it. */
function abreMenuLinha(l, ancora) {
  fechaMenu();
  const m = el('div', 'menu-linha');
  m.id = 'menu';
  m.setAttribute('role', 'menu');
  m.style.setProperty('--cor', corDaLinha(l));
  const item = (txt, acao, extra) => {
    const b = el('button', extra || '', txt);
    b.setAttribute('role', 'menuitem');
    b.onclick = () => { fechaMenu(); acao(); };
    m.append(b);
    return b;
  };
  // Focus is a set: several lines can be in focus at once.
  if (est.minhas.length > 1) {
    item(est.focadas.has(l) ? 'Tirar o foco desta linha' : 'Focar nesta linha', () => focaLinha(l), 'so');
    if (est.focadas.size && !(est.focadas.size === 1 && est.focadas.has(l))) {
      item('Mostrar todas as linhas', () => { est.focadas.clear(); desenha(); });
    }
  }
  const b = est.bundles.get(l);
  const atual = dirAtual(l);
  if (b) {
    const vistos = new Set();
    for (const d of b.dirs) {
      if (vistos.has(d.headsign)) continue;
      vistos.add(d.headsign);
      const escolhido = atual && atual.headsign === d.headsign;
      item(`${escolhido ? '✓ ' : ''}Indo para ${d.headsign}`, () => {
        est.sentido[l] = d.headsign;
        salva();
        desenha();
        atualiza();
      }, escolhido ? 'ativo' : '');
    }
  }
  item('Remover linha', () => segue(l, false), 'perigo');

  document.body.append(m);
  const r = ancora.getBoundingClientRect();
  m.style.left = Math.min(innerWidth - m.offsetWidth - 8, r.right + 10) + 'px';
  m.style.top = '0px';
  // Measured at its final width; near the bottom it opens upward and the
  // arrow still points at the badge that was tapped.
  const h = m.offsetHeight;
  const meio = r.top + r.height / 2;
  const top = Math.max(8, Math.min(innerHeight - h - 8, meio - 22));
  m.style.top = top + 'px';
  m.style.setProperty('--seta', `${Math.max(10, Math.min(h - 22, meio - top - 6))}px`);
  setTimeout(() => addEventListener('pointerdown', fechaMenuFora, { once: true }), 0);
}
function fechaMenuFora(e) { if (!e.target.closest('#menu')) fechaMenu(); }
function fechaMenu() { const m = $('#menu'); if (m) m.remove(); }

/* ---------- stop sheet ---------- */
async function abreFolha(p) {
  est.folha = { id: p.id, lat: p.lat, lon: p.lon, nome: p.nome, sub: p.sub };
  est.aba = 'vivo';
  est.balao = null;
  fechaBusca();
  $('#folha').hidden = false;
  $('#folha').classList.remove('alta');
  document.body.classList.add('folha-aberta');
  desenha();
  await paradasEm(p.lat, p.lon);
  await atualiza();
  enquadraFolha();
}

/* The stop, and the first few buses on their way to it, above the sheet. */
function enquadraFolha() {
  if (!est.folha) return;
  const pts = [[est.folha.lat, est.folha.lon]];
  const todos = [...est.veiculosFolha, ...[...est.frotas.values()].flat()];
  for (const a of chegadasEm(est.folha.id).slice(0, 4)) {
    const v = todos.find(x => x.id === a[7]);
    if (v && a[5] < 3000) pts.push([v.lat, v.lon]);
  }
  enquadraPontos(pts);
  desenhaMapa();
}
function fechaFolha() {
  est.folha = null;
  est.veiculosFolha = [];
  $('#folha').hidden = true;
  document.body.classList.remove('folha-aberta');
  desenha();
}

function linhaHora(grupo) {
  const [primeiro, ...resto] = grupo.lista;
  const f = faixa(primeiro);
  const velho = agoraS() - primeiro[6] > 90;
  const row = el('div', 'hora');
  row.setAttribute('role', 'button');
  row.tabIndex = 0;

  const badge = selo(grupo.linha, 'hora-selo');
  if (est.minhas.includes(grupo.linha)) badge.style.background = corDaLinha(grupo.linha);
  const destino = el('div', 'hora-destino', grupo.destino);
  const meta = el('div', 'hora-meta');
  const est_ = el('button', 'est');
  const seguida = est.minhas.includes(grupo.linha);
  est_.append(icone(seguida ? 'estrela' : 'estrela-vazia'));
  est_.setAttribute('aria-label', seguida ? `Parar de acompanhar ${grupo.linha}` : `Acompanhar ${grupo.linha}`);
  est_.onclick = e => { e.stopPropagation(); segue(grupo.linha, !seguida, grupo.destino); };
  const onde = primeiro[4] === 0 ? `a ${metros(primeiro[5])}`
    : `${primeiro[4] === 1 ? '1 parada' : primeiro[4] + ' paradas'} · ${metros(primeiro[5])}`;
  meta.append(est_, el('span', '', onde));

  const tempo = el('div', 'hora-tempo' + (velho ? ' velho' : ''));
  const g = el('span', 'grande' + (f.chegando ? ' agora' : ''));
  if (f.chegando) g.append(el('b', '', 'chegando'));
  else g.append(el('b', '', `${f.l}`), el('i', '', 'a'), el('b', '', `${f.h}`), el('i', '', 'min'));
  if (!velho) g.append(icone('sinal'));
  tempo.append(g);
  if (resto.length) {
    const r = faixa(resto[0]);
    tempo.append(el('small', '', r.chegando ? 'depois · chegando' : `depois · ${r.l} a ${r.h} min`));
  } else {
    tempo.append(el('small', '', velho ? `GPS ${atras(agoraS() - primeiro[6])}` : ' '));
  }

  row.append(badge, destino, tempo, meta);
  row.onclick = () => { est.sentido[grupo.linha] = grupo.destino; segue(grupo.linha, true, grupo.destino); enquadraLinha(grupo.linha); desenha(); };
  return row;
}

async function desenhaFolha() {
  const box = $('#folha-conteudo');
  if (!est.folha) { box.textContent = ''; return; }
  const p = est.folha;
  box.textContent = '';

  const rot = el('div', 'folha-rotulo');
  rot.append(el('span', '', 'HORÁRIOS DOS ÔNIBUS'));
  const fechar = el('button', 'folha-fechar');
  fechar.append(icone('fechar'));
  fechar.setAttribute('aria-label', 'Fechar');
  fechar.onclick = fechaFolha;
  rot.append(fechar);
  box.append(rot);

  const cab = el('div', 'estacao');
  const ic = el('div', 'estacao-icone');
  ic.append(icone('bus'));
  const nomes = el('div');
  nomes.append(el('div', 'estacao-nome', p.nome));
  const dist = distM(est.eu, [p.lat, p.lon]);
  nomes.append(el('div', 'estacao-sub', [p.sub, `${aPe(dist)} · ${metros(dist)}`].filter(Boolean).join(' · ')));
  cab.append(ic, nomes);
  box.append(cab);

  const seg = el('div', 'segmentos');
  const bVivo = el('button');
  bVivo.append(el('span', 'ponto-vivo'), document.createTextNode('Tempo real'));
  const bTab = el('button', '', 'Tabela de horários');
  bVivo.setAttribute('aria-pressed', est.aba === 'vivo');
  bTab.setAttribute('aria-pressed', est.aba === 'tabela');
  bVivo.onclick = () => { est.aba = 'vivo'; desenhaFolha(); };
  bTab.onclick = () => { est.aba = 'tabela'; desenhaFolha(); };
  seg.append(bVivo, bTab);
  box.append(seg);

  if (est.aba === 'vivo') {
    const grupos = new Map();
    for (const a of chegadasEm(p.id)) {
      const k = a[1] + '\u0000' + a[2];
      if (!grupos.has(k)) grupos.set(k, { linha: a[1], destino: a[2], lista: [] });
      grupos.get(k).lista.push(a);
    }
    if (!grupos.size) {
      box.append(el('div', 'vazio', est.buscadoEm
        ? 'Nenhum ônibus a caminho desta parada agora. Veja na tabela de quanto em quanto tempo cada linha passa.'
        : 'Carregando…'));
    }
    for (const g of grupos.values()) box.append(linhaHora(g));
    box.append(el('p', 'nota',
      '* Tempos estimados pela velocidade atual dos ônibus de cada linha. Podem variar com o trânsito.'));
  } else {
    await desenhaTabela(box, p);
  }
}

async function desenhaTabela(box, p) {
  const info = est.paradas.get(p.id);
  const linhas = info && info.linhas ? info.linhas : [...new Set(est.chegadas.filter(a => a[0] === p.id).map(a => a[1]))];
  if (!linhas.length) { box.append(el('div', 'vazio', 'Sem horários publicados para esta parada.')); return; }
  const lista = el('div');
  box.append(lista);
  const d = new Date();
  const svc = d.getDay() === 0 ? 2 : d.getDay() === 6 ? 1 : 0;
  const nomeDia = ['dia útil', 'sábado', 'domingo'][svc];
  for (const l of linhas.slice(0, 30)) {
    const b = await bundle(l);
    if (!b || est.aba !== 'tabela' || est.folha?.id !== p.id) continue;
    for (const dir of b.dirs) {
      if (!dir.stops.some(s => s[0] === p.id)) continue;
      const bloco = el('div', 'freq-linha');
      const topo = el('div', 'freq-topo');
      topo.append(selo(l), el('b', '', dir.headsign));
      const iv = intervaloAgora(dir);
      topo.append(el('span', 'freq-agora', iv ? `a cada ~${iv} min` : 'sem serviço agora'));
      bloco.append(topo);
      const faixas = el('div', 'freq-faixas');
      const h = d.getHours();
      for (const f of dir.freq.filter(x => x[0] === svc)) {
        const s = el('span', h >= f[1] && h < f[2] ? 'atual' : '', `${f[1]}h às ${f[2]}h ~${f[3]} min`);
        faixas.append(s);
      }
      if (!faixas.children.length) faixas.append(el('span', '', `sem serviço em ${nomeDia}`));
      bloco.append(faixas);
      lista.append(bloco);
    }
  }
  box.append(el('p', 'nota', `* Intervalo médio publicado pela SMTR para ${nomeDia}. Os horários podem sofrer variações devido ao trânsito, fins de semana e feriados.`));
}

function ligaFolha() {
  const pega = $('#pega');
  const f = $('#folha');
  let ini = null;
  pega.addEventListener('pointerdown', e => {
    ini = { y: e.clientY, h: f.getBoundingClientRect().height, mov: 0 };
    pega.setPointerCapture(e.pointerId);
    f.classList.add('arrastando');
  });
  pega.addEventListener('pointermove', e => {
    if (!ini) return;
    ini.mov = e.clientY - ini.y;
    f.style.height = Math.max(120, ini.h - ini.mov) + 'px';
  });
  pega.addEventListener('pointerup', () => {
    if (!ini) return;
    f.classList.remove('arrastando');
    f.style.height = '';
    if (ini.mov > 120) fechaFolha();
    else if (ini.mov < -40) f.classList.add('alta');
    else if (ini.mov > 40) f.classList.remove('alta');
    else if (Math.abs(ini.mov) < 6) f.classList.toggle('alta');
    ini = null;
  });
}

/* ---------- search ---------- */
function abreBusca() {
  est.buscando = true;
  document.body.classList.add('buscando');
  if (!est.freq) carregaFreq().then(() => { if (est.buscando) desenhaSugestoes(); });
  est.balao = null;
  $('#sugestoes').hidden = false;
  const ic = $('#barra-icone');
  ic.textContent = '';
  ic.append(icone('voltar'));
  ic.setAttribute('aria-label', 'Fechar busca');
  desenhaSugestoes();
}
function fechaBusca() {
  if (!est.buscando) return;
  est.buscando = false;
  document.body.classList.remove('buscando');
  est.busca = '';
  $('#campo').value = '';
  $('#campo').blur();
  $('#sugestoes').hidden = true;
  const ic = $('#barra-icone');
  ic.textContent = '';
  ic.append(icone('bus'));
  ic.setAttribute('aria-label', 'Buscar linha');
}

function origemDestino(c) {
  // "Irajá - Castelo" reads as "Irajá x Castelo", the way the incumbent lists lines.
  return (c[1] || c[3].join(' - ')).replace(/\s+-\s+/g, ' x ');
}

function sug(nome, legenda) {
  const c = est.catalogo.find(x => x[0] === nome);
  const row = el('div', 'sug');
  row.setAttribute('role', 'button');
  row.tabIndex = 0;
  const seguida = est.minhas.includes(nome);
  const e = el('button', 'sug-estrela');
  e.append(icone(seguida ? 'estrela' : 'estrela-vazia'));
  e.setAttribute('aria-label', seguida ? `Parar de acompanhar ${nome}` : `Acompanhar ${nome}`);
  e.onclick = ev => { ev.stopPropagation(); segue(nome, !seguida); desenhaSugestoes(); };
  const badge = selo(nome);
  if (seguida) badge.style.background = corDaLinha(nome);
  const t = el('div', 'sug-txt');
  const titulo = el('div');
  const txt = c ? origemDestino(c) : nome;
  const q = est.busca.trim();
  const i = q ? txt.toLowerCase().indexOf(q.toLowerCase()) : -1;
  if (i >= 0) {
    titulo.append(txt.slice(0, i), el('b', '', txt.slice(i, i + q.length)), txt.slice(i + q.length));
  } else titulo.textContent = txt;
  t.append(titulo);
  if (legenda) t.append(el('small', '', legenda));
  row.append(e, badge, t);
  row.onclick = async () => { fechaBusca(); await segue(nome, true); enquadraLinha(nome); desenha(); };
  return row;
}

async function carregaFreq() {
  if (est.freq) return est.freq;
  est.freq = await pega(`${EST}/freq.json`).catch(() => ({}));
  return est.freq;
}

/* Scheduled headway of a line for this hour, in minutes, or null when it does
   not run now. */
function freqAgora(l) {
  const f = est.freq && est.freq[l];
  if (!f) return null;
  const d = new Date();
  const svc = d.getDay() === 0 ? 2 : d.getDay() === 6 ? 1 : 0;
  const h = d.getHours();
  const r = f.find(x => x[0] === svc && h >= x[1] && h < x[2]);
  return r ? r[3] : null;
}

/* Lines near you, best first. "Best" is the time until you are on board: the
   walk to the stop (4.5 km/h) plus the average wait, half the headway. A line
   every 3 min at 400 m beats one every 20 min at your door, and a frequent
   line two kilometres away does not appear at all. */
function linhasPerto() {
  const m = new Map();
  for (const s of est.paradas.values()) {
    if (!s.linhas) continue;
    const d = distM(est.eu, [s.lat, s.lon]);
    if (d > RAIO_PERTO) continue;
    for (const l of s.linhas) if (!m.has(l) || d < m.get(l)) m.set(l, d);
  }
  return [...m].map(([l, d]) => {
    const h = freqAgora(l);
    return { l, d, h, custo: d / 75 + (h == null ? 1e3 : h / 2) };
  }).sort((a, b) => a.custo - b.custo);
}

function desenhaSugestoes() {
  const box = $('#sugestoes');
  if (!est.buscando) return;
  box.textContent = '';
  const q = est.busca.trim().toLowerCase();
  if (q) {
    const hits = est.catalogo.filter(c =>
      c[0].toLowerCase().startsWith(q) || c[1].toLowerCase().includes(q) ||
      c[3].some(d => d.toLowerCase().includes(q))).slice(0, 30);
    if (!hits.length) box.append(el('div', 'sug-vazio', `Nenhuma linha com "${est.busca}".`));
    for (const c of hits) box.append(sug(c[0]));
    return;
  }
  if (est.minhas.length) {
    box.append(el('div', 'sug-grupo', 'Suas linhas'));
    for (const l of est.minhas) box.append(sug(l));
  }
  const perto = linhasPerto().filter(x => !est.minhas.includes(x.l));
  box.append(el('div', 'sug-grupo', perto.length ? 'Perto de você' : 'Nenhuma linha perto de você'));
  for (const x of perto.slice(0, 25)) {
    const quando = x.h == null ? 'sem ônibus nesta hora' : `a cada ~${x.h} min`;
    box.append(sug(x.l, `${quando} · ${aPe(x.d)}`));
  }
}

/* ---------- following ---------- */
async function segue(l, sim, headsign) {
  if (sim && !est.minhas.includes(l)) {
    if (est.minhas.length >= 8) est.minhas.shift();
    est.minhas.push(l);
    if (headsign) est.sentido[l] = headsign;
    salva();
    await bundle(l);
    desenha();
    await atualiza();
  } else if (!sim) {
    est.minhas = est.minhas.filter(x => x !== l);
    est.frotas.delete(l);
    est.focadas.delete(l);
    salva();
    desenha();
  } else if (headsign) {
    est.sentido[l] = headsign;
    salva();
    desenha();
  }
}

/* Removes every line at once. */
function removeTodas() {
  fechaMenu();
  if (!est.minhas.length) return;
  est.minhas = [];
  est.focadas.clear();
  est.frotas.clear();
  est.balao = null;
  salva();
  desenha();
}

/* Adds a line to the focus, or takes it out. Focusing frames the line. */
function focaLinha(l) {
  if (est.focadas.has(l)) est.focadas.delete(l);
  else { est.focadas.add(l); enquadraLinha(l); }
  desenha();
}

/* Frame you, the stop you would use, and the buses on their way to it. */
function enquadraLinha(l) {
  const d = dirAtual(l);
  if (!d) return;
  const p = paradaPerto(d);
  const pts = [est.eu];
  if (p) pts.push([p.lat, p.lon]);
  if (p) for (const a of chegadasEm(p.id, l, d.headsign).slice(0, 2)) {
    const v = (est.frotas.get(l) || []).find(x => x.id === a[7]);
    if (v) pts.push([v.lat, v.lon]);
  }
  enquadraPontos(pts);
}

/* ---------- bus callout ---------- */
function desenhaBalao() {
  const b = $('#balao');
  if (!est.balao) { b.hidden = true; return; }
  const v = est.balao;
  const pos = naTela(v.lat, v.lon);
  b.hidden = false;
  b.textContent = '';
  const corpo = el('div', 'balao-corpo');
  const lb = el('div', 'balao-linha');
  lb.style.setProperty('--cor', v.cor);
  lb.append(el('b', '', v.line), el('small', '', v.id));
  const info = el('div', 'balao-info');
  const s1 = el('div', 'sinal');
  s1.append(icone('sinal'), document.createTextNode(atras(agoraS() - v.t)));
  info.append(s1, el('div', '', `${Math.round(v.spd || 0)} km/h`));
  corpo.append(lb, info);
  b.append(corpo);
  const rod = el('div', 'balao-rodape');
  rod.append(document.createTextNode(v.dir ? `Sentido ${v.to || v.dir}` : 'Sentido ainda não confirmado'));
  const a = est.chegadas.find(x => x[7] === v.id && (!est.folha || x[0] === est.folha.id));
  if (a) {
    const f = faixa(a);
    rod.append(document.createTextNode(' · '), el('b', '', f.chegando ? 'chegando' : `chega ${textoFaixa(f)}`));
  }
  b.append(rod);

  // Keep the whole callout on screen: shift it sideways near an edge and open
  // it below the bus near the top, with the arrow still on the bus.
  const w = b.offsetWidth, h = b.offsetHeight;
  const left = Math.max(8, Math.min(innerWidth - w - 8, pos[0] - w / 2));
  const abaixo = pos[1] - h - 22 < 76;
  b.style.left = left + 'px';
  b.style.top = (abaixo ? pos[1] + 22 : pos[1] - h - 22) + 'px';
  b.style.setProperty('--seta-x', `${Math.max(14, Math.min(w - 14, pos[0] - left))}px`);
  b.classList.toggle('abaixo', abaixo);
}


/* Called by the map when a bus or a stop is tapped. */
function aoTocarMapa(alvo) {
  fechaMenu();
  if (est.buscando) { fechaBusca(); return; }
  if (!alvo) { if (est.balao) { est.balao = null; desenhaBalao(); } return; }
  if (alvo.tipo === 'onibus') {
    const a = est.chegadas.find(x => x[7] === alvo.v.id);
    est.balao = { ...alvo.v, cor: alvo.cor, to: a ? a[2] : null };
    desenhaBalao();
  } else if (alvo.tipo === 'parada') {
    abreFolha(alvo.p);
  }
}

/* ---------- settings ---------- */
function abreAjustes() {
  const box = $('#ajustes-lista');
  box.textContent = '';
  const itens = [
    ['escuro', 'Modo escuro', ''],
    ['rota', 'Exibir rota', 'O traçado das linhas que você acompanha'],
    ['paradas', 'Exibir paradas', 'Pontos de ônibus no mapa'],
    ['semSinal', 'Exibir ônibus sem sinal', 'Ônibus que não enviam posição há mais de 3 minutos'],
  ];
  for (const [k, titulo, sub] of itens) {
    const b = el('button', 'ajuste');
    b.setAttribute('role', 'switch');
    b.setAttribute('aria-checked', !!est.ajustes[k]);
    const t = el('div', 'ajuste-txt', titulo);
    if (sub) t.append(el('small', '', sub));
    b.append(t, el('span', 'chave'));
    b.onclick = () => {
      est.ajustes[k] = !est.ajustes[k];
      salva();
      aplicaTema();
      abreAjustes();
      desenha();
    };
    box.append(b);
  }
  $('#ajustes').hidden = false;
}
function aplicaTema() {
  document.documentElement.dataset.tema = est.ajustes.escuro ? 'escuro' : 'claro';
  document.querySelector('meta[name=theme-color]').content = est.ajustes.escuro ? '#202124' : '#ffffff';
}

/* ---------- map scene ---------- */
/* What the map shows is decided by what you are doing, not by what is known.
 * By default: the route of each line you follow, only in the direction you
 * ride; the stop you would walk to; and the next few buses on their way to
 * it. Everything else waits for a reason to appear: focusing a line shows all
 * of its buses and its stops, opening a stop shows the buses coming to it,
 * and zooming in shows the stops around you. */
const MAX_POR_LINHA = 3;

function onibusIndoParaMim(l) {
  const d = dirAtual(l);
  const p = d && paradaPerto(d);
  if (!p) return [];
  const frota = est.frotas.get(l) || [];
  return chegadasEm(p.id, l, d.headsign)
    .slice(0, MAX_POR_LINHA)
    .map(a => frota.find(v => v.id === a[7]))
    .filter(Boolean);
}

function cena() {
  const c = { rotas: [], suas: [], pontinhos: [], outras: [], onibus: [], destaque: null };
  const velhoOk = est.ajustes.semSinal;
  const foco = est.focadas;
  const vistos = new Set();
  const addOnibus = (v, cor) => {
    if (vistos.has(v.id) || v.ph === 'parked') return;
    const velho = agoraS() - v.t > 180;
    if (velho && !velhoOk) return;
    vistos.add(v.id);
    c.onibus.push({ v, cor, velho });
  };

  for (const l of est.minhas) {
    if (foco.size && !foco.has(l)) continue;
    const b = est.bundles.get(l);
    const d = dirAtual(l);
    if (!b || !d) continue;
    const cor = corDaLinha(l);
    if (est.ajustes.rota) c.rotas.push({ cor, pts: d.pts, forte: foco.has(l) });
    const p = paradaPerto(d);
    if (p && !est.folha) {
      // Several lines can share the stop you would use: one sign, one label.
      const ja = c.suas.find(s => s.id === p.id);
      if (ja) ja.linhas.push(l);
      else c.suas.push({ ...p, cor, linhas: [l] });
    }
    // The stops of your lines only, never every stop in the area: with lines
    // in focus, only theirs. Small dots in the line's colour, from zoom 15.
    if (est.ajustes.paradas && mapa.z >= 15 && !est.folha) {
      for (const s of d.stops) {
        if (p && s[0] === p.id) continue;
        c.pontinhos.push({ lat: s[1], lon: s[2], cor, id: s[0], nome: s[3], sub: s[5] || '' });
      }
    }
    if (foco.has(l)) {
      // In focus: every bus riding this direction, not just the next few.
      for (const v of est.frotas.get(l) || []) if (v.shp === d.shape) addOnibus(v, cor);
    } else if (!est.folha) {
      for (const v of onibusIndoParaMim(l)) addOnibus(v, cor);
    }
  }

  if (est.folha) {
    // A stop is open: it, and the buses on their way to it.
    c.destaque = est.folha;
    const frotas = [...est.frotas.values()].flat();
    for (const a of chegadasEm(est.folha.id).slice(0, 8)) {
      const v = frotas.find(x => x.id === a[7]) || est.veiculosFolha.find(x => x.id === a[7]);
      if (v) addOnibus({ ...v, line: a[1] }, corDaLinha(a[1]));
    }
  }
  return c;
}

/* ---------- render ---------- */
function desenha() {
  try {
    desenhaTrilho();
    if (est.folha) desenhaFolha();
    if (est.buscando) desenhaSugestoes();
  } catch (e) {
    console.error(e);
  }
  desenhaMapa();
  desenhaBalao();
}

/* ---------- where you are ----------
 * GPS, a place picked on the map, or a neighbourhood or stop found by name.
 * A place you chose is remembered, as onibus-rj did, so opening the app at the
 * same stop tomorrow needs no GPS at all. Names come from our own data (1,011
 * neighbourhoods, 7,694 stops), so there is no geocoding service to call. */
const norm = s => String(s).normalize('NFD').replace(/[\u0300-\u036f]/g, '').toLowerCase();

function salvaLocal() {
  try {
    localStorage.setItem('busones:local', JSON.stringify(
      est.local.modo === 'manual' ? { modo: 'manual', lat: est.eu[0], lon: est.eu[1], nome: est.local.nome } : { modo: est.local.modo }));
  } catch {}
}
function localSalvo() {
  try { return JSON.parse(localStorage.getItem('busones:local') || 'null'); } catch { return null; }
}

function atualizaOnde() {
  const t = $('#onde-txt');
  if (!t) return;
  t.textContent = est.local.modo === 'manual' ? est.local.nome
    : est.local.modo === 'gps' ? 'Minha localização' : 'Onde você está?';
}

async function definirLocal(lat, lon, nome, modo, opts = {}) {
  est.eu = [lat, lon];
  est.temLocal = true;
  est.local = { modo, nome };
  salvaLocal();
  atualizaOnde();
  if (opts.centraliza !== false) {
    mapa.centro = [lat, lon];
    mapa.z = Math.max(mapa.z, 15);
  }
  desenha();
  if (opts.endereco) nomeEndereco(lat, lon);
  await paradasEm(lat, lon);
  desenha();
  await atualiza();
}

/* The street and number of a point, as onibus-rj showed it; the local name
   (nearest stop or neighbourhood) stays until the answer arrives. */
let reversoSeq = 0;
async function nomeEndereco(lat, lon) {
  const seq = ++reversoSeq;
  try {
    const r = await fetch(`${GEO}/reverse?lat=${lat.toFixed(5)}&lon=${lon.toFixed(5)}`);
    if (!r.ok) return;
    const l = await r.json();
    if (!l || !l.nome || seq !== reversoSeq || est.local.modo !== 'manual') return;
    if (Math.abs(est.eu[0] - lat) > 1e-6 || Math.abs(est.eu[1] - lon) > 1e-6) return;
    est.local.nome = l.nome;
    salvaLocal();
    atualizaOnde();
  } catch {}
}

/* Called by the map: the pin was dragged, or a point was held down. */
async function aoMoverPino(lat, lon) {
  await paradasEm(lat, lon);
  definirLocal(lat, lon, nomePerto(lat, lon), 'manual', { centraliza: false, endereco: true });
}

function usarGps(silencioso) {
  if (!navigator.geolocation) { if (!silencioso) avisoLocal('Este navegador não informa a localização.'); return; }
  navigator.geolocation.getCurrentPosition(
    p => { fechaLocal(); definirLocal(p.coords.latitude, p.coords.longitude, 'Minha localização', 'gps'); },
    () => {
      if (silencioso && !est.temLocal) abreLocal('Não conseguimos sua localização. Escolha onde você está.');
      else if (!silencioso) avisoLocal('Não foi possível usar o GPS. Escolha no mapa ou busque um bairro ou parada.');
    },
    { enableHighAccuracy: true, timeout: 8000, maximumAge: 30000 });
}

function nomePerto(lat, lon) {
  let parada = null, dp = Infinity;
  for (const s of est.paradas.values()) {
    const d = distM([lat, lon], [s.lat, s.lon]);
    if (d < dp) { dp = d; parada = s; }
  }
  if (parada && dp <= 250) return parada.nome;
  let bairro = null, db = Infinity;
  for (const p of lugares || []) {
    const d = distM([lat, lon], [p[1], p[0]]);
    if (d < db) { db = d; bairro = p[2]; }
  }
  return bairro && db < 3000 ? bairro : 'Local escolhido';
}

async function carregaTodasParadas() {
  if (est.todasParadas) return est.todasParadas;
  est.todasParadas = await pega(`${EST}/stops.json`).catch(() => []);
  for (const s of est.todasParadas) s.n = norm(s[3]);
  return est.todasParadas;
}

function linhaLugar(icone_, cls, titulo, sub, acao, atual) {
  const b = el('button', 'lugar' + (atual ? ' atual' : ''));
  const ic = el('span', 'lugar-icone ' + cls);
  ic.append(icone(icone_));
  const t = el('span', 'lugar-txt');
  t.append(el('b', '', titulo));
  if (sub) t.append(el('small', '', sub));
  b.append(ic, t);
  b.onclick = acao;
  return b;
}

let buscaLocalSeq = 0;
async function desenhaLocal(aviso) {
  const lista = $('#local-lista');
  const q = norm($('#local-campo').value.trim());
  const seq = ++buscaLocalSeq;
  lista.textContent = '';
  if (aviso) lista.append(el('p', 'local-aviso', aviso));
  if (!q) {
    lista.append(
      linhaLugar('mira', 'gps', 'Usar minha localização', 'Pelo GPS do aparelho', () => usarGps(false), est.local.modo === 'gps'),
      linhaLugar('pino', 'mapa', 'Escolher no mapa', 'Ou arraste o alfinete, ou segure o dedo num ponto do mapa', iniciaEscolha, false));
    if (est.local.modo === 'manual') {
      lista.append(linhaLugar('pino', '', est.local.nome, 'Local escolhido por você',
        () => { fechaLocal(); mapa.centro = [...est.eu]; desenha(); }, true));
    }
    return;
  }
  carregaLugares();
  // Full addresses come from the backend on request, never per keystroke:
  // Nominatim's usage policy forbids search-as-you-type.
  if (q.length >= 3) {
    const bruto = $('#local-campo').value.trim();
    lista.append(linhaLugar('busca', 'gps', `Buscar endereço "${bruto}"`, 'Rua e número, ou um lugar',
      () => buscaEndereco(bruto), false));
  }
  // One entry per name: two stops called "Metrô Glória" are the two sides of
  // the same street, and either is a fine answer to "where are you".
  const unicos = (lista, nome) => {
    const vistos = new Set();
    return lista.filter(x => { const k = norm(nome(x)); if (vistos.has(k)) return false; vistos.add(k); return true; });
  };
  const bairros = unicos((lugares || [])
    .filter(p => norm(p[2]).includes(q))
    .sort((x, y) => (norm(x[2]).startsWith(q) ? 0 : 1) - (norm(y[2]).startsWith(q) ? 0 : 1) || x[3] - y[3]), p => p[2])
    .slice(0, 6);
  for (const p of bairros) {
    lista.append(linhaLugar('pino', '', p[2], p[3] === 2 ? 'Bairro' : 'Região',
      () => { fechaLocal(); definirLocal(p[1], p[0], p[2], 'manual'); }, false));
  }
  const paradas = await carregaTodasParadas();
  if (seq !== buscaLocalSeq) return;
  const achadas = unicos(paradas.filter(s => s.n.includes(q))
    .sort((x, y) => (x.n.startsWith(q) ? 0 : 1) - (y.n.startsWith(q) ? 0 : 1)), s => s[3]).slice(0, 12);
  for (const s of achadas) {
    lista.append(linhaLugar('parada', 'parada', s[3], s[4] ? `Parada · ${s[4]}` : 'Parada',
      () => { fechaLocal(); definirLocal(s[1], s[2], s[3], 'manual'); }, false));
  }
  if (!bairros.length && !achadas.length) lista.append(el('p', 'local-aviso', `Nada encontrado para "${$('#local-campo').value}".`));
}

async function buscaEndereco(texto) {
  const lista = $('#local-lista');
  const seq = ++buscaLocalSeq;
  lista.textContent = '';
  lista.append(el('p', 'local-aviso', 'Buscando endereço…'));
  try {
    const r = await fetch(`${GEO}/geocode?q=${encodeURIComponent(texto)}`);
    if (seq !== buscaLocalSeq) return;
    const lugaresAchados = r.ok ? await r.json() : null;
    lista.textContent = '';
    if (!lugaresAchados) {
      lista.append(el('p', 'local-aviso', 'A busca de endereço não respondeu agora. Tente bairro ou parada, ou escolha no mapa.'));
      return;
    }
    if (!lugaresAchados.length) {
      lista.append(el('p', 'local-aviso', `Nenhum endereço encontrado para "${texto}" no Rio.`));
      return;
    }
    for (const l of lugaresAchados) {
      lista.append(linhaLugar('pino', '', l.nome, l.area || 'Endereço',
        () => { fechaLocal(); definirLocal(l.lat, l.lon, l.nome, 'manual'); }, false));
    }
  } catch {
    if (seq === buscaLocalSeq) {
      lista.textContent = '';
      lista.append(el('p', 'local-aviso', 'Sem conexão com a busca de endereço.'));
    }
  }
}

function abreLocal(aviso) {
  fechaBusca();
  fechaMenu();
  est.balao = null;
  $('#local-campo').value = '';
  $('#local').hidden = false;
  document.body.classList.add('local-aberto');
  carregaLugares();
  desenhaLocal(aviso);
}
function fechaLocal() { $('#local').hidden = true; document.body.classList.remove('local-aberto'); }
function avisoLocal(txt) { if ($('#local').hidden) abreLocal(txt); else desenhaLocal(txt); }

function iniciaEscolha() {
  fechaLocal();
  fechaFolha();
  est.escolhendoLocal = true;
  document.body.classList.add('escolhendo');
  $('#escolha').hidden = false;
  if (mapa.z < 15) mapa.z = 15;
  desenha();
}
function terminaEscolha() {
  est.escolhendoLocal = false;
  document.body.classList.remove('escolhendo');
  $('#escolha').hidden = true;
  desenha();
}
async function confirmaEscolha() {
  const [lat, lon] = mapa.centro;
  terminaEscolha();
  await paradasEm(lat, lon);
  definirLocal(lat, lon, nomePerto(lat, lon), 'manual', { endereco: true });
}

/* ---------- boot ---------- */
async function inicia() {
  carrega();
  const q = new URLSearchParams(location.search);
  const salvo = localSalvo();
  if (q.get('em')) {
    const [la, lo] = q.get('em').split(',').map(Number);
    if (isFinite(la) && isFinite(lo)) { est.eu = [la, lo]; est.temLocal = true; est.local = { modo: 'manual', nome: 'Local escolhido' }; }
  } else if (salvo && salvo.modo === 'manual' && isFinite(salvo.lat)) {
    est.eu = [salvo.lat, salvo.lon];
    est.temLocal = true;
    est.local = { modo: 'manual', nome: salvo.nome || 'Local escolhido' };
  }
  if (q.get('linhas')) { est.minhas = q.get('linhas').split(',').filter(Boolean).slice(0, 8); salva(); }
  aplicaTema();
  mapa.centro = [...est.eu];
  mapa.z = 15;
  ligaFolha();

  const campo = $('#campo');
  campo.addEventListener('focus', abreBusca);
  campo.addEventListener('input', () => { est.busca = campo.value; desenhaSugestoes(); });
  campo.addEventListener('keydown', e => { if (e.key === 'Escape') fechaBusca(); });
  $('#barra-icone').onclick = () => est.buscando ? fechaBusca() : campo.focus();
  $('#lupa').onclick = () => campo.focus();
  $('#fab-local').onclick = () => usarGps(false);
  $('#onde').onclick = () => abreLocal();
  $('#local-fechar').onclick = fechaLocal;
  $('#local-campo').addEventListener('input', () => desenhaLocal());
  $('#local-campo').addEventListener('keydown', e => {
    const v = $('#local-campo').value.trim();
    if (e.key === 'Enter' && v.length >= 3) buscaEndereco(v);
  });
  $('#escolha-cancelar').onclick = terminaEscolha;
  $('#escolha-ok').onclick = confirmaEscolha;
  $('#engrenagem').onclick = abreAjustes;
  $('#ajustes-voltar').onclick = () => { $('#ajustes').hidden = true; };

  est.catalogo = await pega(`${EST}/lines.json`).catch(() => []);
  await paradasEm(est.eu[0], est.eu[1]);
  await Promise.all(est.minhas.map(bundle));
  desenha();
  await atualiza();
  if (q.get('parada')) {
    const p = est.paradas.get(q.get('parada'));
    if (p) abreFolha(p);
  }
  if (q.get('busca') != null) {
    est.busca = q.get('busca');
    $('#campo').value = est.busca;
    abreBusca();
  }
  atualizaOnde();
  if (q.get('escolher')) iniciaEscolha();
  else if (!q.get('em') && est.local.modo !== 'manual') usarGps(true);
  setInterval(atualiza, RECARGA_MS);
  setInterval(desenhaBalao, 5000);
}
