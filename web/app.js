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
const STATIC = '/dist';
const Z = 13;
const NEARBY_RADIUS = 600;
const REFRESH_MS = 12000;
const CITY_CENTER = [-22.9068, -43.1729];

/* For the few lines the GTFS gives no colour, and lines without a route. */
const FALLBACK_COLOR = '#546E7A';

/* The badge used in lists: always the same width, so destinations line up in
   one column whatever the code ("3" or "LECD148"); the type shrinks instead. */
function listBadge(l, extra = '') {
  const s = el('span', 'badge' + (extra ? ' ' + extra : ''), l);
  s.dataset.size = String(Math.min(7, Math.max(4, l.length)));
  s.style.background = lineColor(l);
  s.style.color = lineTextColor(l);
  return s;
}

/* The line badge from onibus-rj: a rounded square with the number, the type
   shrinking as the code gets longer ("474" to "LECD133"). */
function lineBadge(l, tag = 'div') {
  const n = el(tag, 'line-badge', l);
  n.style.setProperty('--color', lineColor(l));
  n.style.setProperty('--color-text', lineTextColor(l));
  n.dataset.size = l.length <= 4 ? 's' : l.length <= 6 ? 'm' : 'l';
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
function icon(name) {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  const u = document.createElementNS('http://www.w3.org/2000/svg', 'use');
  u.setAttribute('href', '#i-' + name);
  s.append(u);
  return s;
}
const meters = m => m >= 1000 ? (m / 1000).toFixed(1).replace('.', ',') + ' km' : Math.round(m) + ' m';
const walkTime = m => `${Math.max(1, Math.round(m / 75))} min a pé`;
/* The incumbent writes data age as "52 seg atrás" and "1:18 atrás". */
function ago(tabs) {
  tabs = Math.max(0, Math.round(tabs));
  if (tabs < 60) return `${tabs} seg atrás`;
  return `${Math.floor(tabs / 60)}:${String(tabs % 60).padStart(2, '0')} atrás`;
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
function decodePolyline(str) {
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
let clockSkew = 0;
const nowS = () => Date.now() / 1000 + clockSkew;
async function getJson(url) {
  const r = await fetch(url, { cache: 'no-cache' });
  if (!r.ok) throw new Error(`${r.status} ${url}`);
  const d = Date.parse(r.headers.get('date') || '');
  if (isFinite(d)) {
    const delta = d / 1000 - Date.now() / 1000;
    if (Math.abs(delta) > 2) clockSkew = delta;
  }
  return r.json();
}

/* ---------- state ---------- */
const state = {
  here: [...CITY_CENTER],
  hasPlace: false,
  myLines: [],
  chosenDir: {},
  catalog: [],
  /* Official colours: line -> [background, text]. */
  colors: new Map(),
  /* Which service runs on which date, from the GTFS calendar. */
  calendar: null,
  /* Lines on the road now: line -> [buses, has a route]. */
  liveLines: new Map(),
  liveLinesAt: 0,
  bundles: new Map(),
  fleets: new Map(),
  arrivals: [],
  stops: new Map(),
  stopCells: new Set(),
  sheet: null,
  tab: 'live',
  sheetVehicles: [],
  query: '',
  searching: false,
  callout: null,
  settings: { dark: false, route: true, stops: true, staleBuses: true },
  place: { mode: null, name: '' },
  /* Lines in focus. Empty means every line you follow is on the map. */
  focused: new Set(),
  freq: null,
  pickingPlace: false,
  allStops: null,
  fetchedAt: 0,
  errorMsg: '',
};

function saveState() {
  try {
    localStorage.setItem('busones:lines', JSON.stringify(state.myLines));
    localStorage.setItem('busones:directions', JSON.stringify(state.chosenDir));
    localStorage.setItem('busones:settings', JSON.stringify(state.settings));
  } catch {}
}
function loadState() {
  try {
    const l = JSON.parse(localStorage.getItem('busones:lines') || '[]');
    if (Array.isArray(l)) state.myLines = l.filter(x => typeof x === 'string').slice(0, 8);
    state.chosenDir = JSON.parse(localStorage.getItem('busones:directions') || '{}') || {};
    const a = JSON.parse(localStorage.getItem('busones:settings') || 'null');
    if (a) Object.assign(state.settings, a);
    else state.settings.dark = matchMedia('(prefers-color-scheme: dark)').matches;
  } catch {}
}

/* ---------- line colours ----------
 * Every line has one fixed colour: the official one from the GTFS. In Rio it
 * is the stripe of the line's operating region on the new yellow buses (grey
 * for Tijuca, Centro and Zona Sul, orange for Campo Grande, pink for
 * Jacarepaguá...), the corridor colour for the BRT and navy for the executive
 * lines. Badges use it as published. The map only shifts its lightness until
 * it stands out from the background: the official grey would vanish among the
 * streets of a light map, and the navy on a dark one. */
function lineColor(l) {
  const c = state.colors.get(l);
  return (c && c[0]) || FALLBACK_COLOR;
}
function lineTextColor(l) {
  const c = state.colors.get(l);
  if (c && c[0] && c[1]) return c[1];
  return luminance(lineColor(l)) > 0.35 ? '#000000' : '#FFFFFF';
}

const rgb = hex => { const n = parseInt(hex.slice(1), 16); return [n >> 16 & 255, n >> 8 & 255, n & 255]; };
function luminance(hex) {
  const [r, g, b] = rgb(hex).map(v => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}
function contrast(a, b) {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}
const contrastCache = new Map();
/* The colour, mixed with black (on a light background) or white (on a dark
   one) in small steps until it reaches the minimum contrast. Hue is kept, so
   the grey line is still grey and the orange one still orange. */
function ensureContrast(color, bg, minRatio) {
  const k = `${color}|${bg}|${minRatio}`;
  if (contrastCache.has(k)) return contrastCache.get(k);
  let res = color;
  if (contrast(color, bg) < minRatio) {
    const towards = luminance(bg) > 0.5 ? 0 : 255;
    const base = rgb(color);
    res = towards ? '#FFFFFF' : '#000000';
    for (let t = 0.05; t < 1; t += 0.05) {
      const hex = '#' + base.map(v => Math.round(v + (towards - v) * t).toString(16).padStart(2, '0')).join('');
      if (contrast(hex, bg) >= minRatio) { res = hex; break; }
    }
  }
  contrastCache.set(k, res);
  return res;
}
/* A line's colour for routes, stops and buses on the map. */
function mapColor(l) {
  return ensureContrast(lineColor(l), dark() ? '#1C1D20' : '#F4F4F1', 3);
}
/* A line's colour as text on the app's own background, as in the line menu. */
function readableColor(l) {
  return ensureContrast(lineColor(l), dark() ? '#202124' : '#FFFFFF', 4.5);
}

/* ---------- data ---------- */
async function bundle(l) {
  if (state.bundles.has(l)) return state.bundles.get(l);
  const b = await getJson(`${STATIC}/lines/${encodeURIComponent(l)}.json`).catch(() => null);
  if (b) {
    for (const d of b.dirs) {
      d.pts = decodePolyline(d.poly);
      for (const s of d.stops) {
        if (!state.stops.has(s[0])) state.stops.set(s[0], { id: s[0], lat: s[1], lon: s[2], name: s[3], sub: s[5] || '', lines: null });
      }
    }
  }
  state.bundles.set(l, b);
  return b;
}

/* How many buses each line has on the road, at most every 30 s. Lets the
   search and the line menu say "3 ônibus agora" or "nenhum ônibus com sinal"
   instead of leaving the screen silent, and finds lines whose route the SMTR
   has not published. */
async function loadLiveLines(force) {
  if (!force && Date.now() - state.liveLinesAt < 30000) return;
  const l = await getJson(`${API}/live-lines.json`).catch(() => null);
  if (!l) return;
  state.liveLines = new Map(l.map(([line, n, route]) => [line, [n, !!route]]));
  state.liveLinesAt = Date.now();
}

async function loadStopsAround(lat, lon) {
  const [x, y] = tile(lat, lon);
  const requests = [];
  for (let dx = -1; dx <= 1; dx++) for (let dy = -1; dy <= 1; dy++) {
    const k = `${x + dx}/${y + dy}`;
    if (state.stopCells.has(k)) continue;
    state.stopCells.add(k);
    requests.push(getJson(`${STATIC}/cells/${Z}/${k}/stops.json`).catch(() => []));
  }
  for (const l of await Promise.all(requests)) {
    for (const s of l) state.stops.set(s[0], { id: s[0], lat: s[1], lon: s[2], name: s[3], sub: s[4] || '', lines: s[5] });
  }
}

/* For one direction of a line, the stop nearest to you. */
function nearestStop(dir) {
  let m = null;
  for (const s of dir.stops) {
    const d = distM(state.here, [s[1], s[2]]);
    if (!m || d < m.dist) m = { id: s[0], lat: s[1], lon: s[2], name: s[3], sub: s[5] || '', dist: d };
  }
  return m;
}

/* The direction a followed line is shown in: the one you last picked, or the
   one whose stop is closest to you. Lá vem hides this behind "mudar sentido";
   here it is always written on screen. */
function currentDir(l) {
  const b = state.bundles.get(l);
  if (!b || !b.dirs.length) return null;
  const requested = state.chosenDir[l];
  const d = b.dirs.find(x => x.headsign === requested);
  if (d) return d;
  return [...b.dirs].sort((a, c) => (nearestStop(a)?.dist ?? 1e9) - (nearestStop(c)?.dist ?? 1e9))[0];
}

async function refresh() {
  try {
    const cells = new Set();
    const add = (la, lo) => { const [x, y] = tile(la, lo); cells.add(`${x}/${y}`); };
    add(state.here[0], state.here[1]);
    for (const l of state.myLines) {
      const b = await bundle(l);
      if (b) for (const d of b.dirs) { const p = nearestStop(d); if (p) add(p.lat, p.lon); }
    }
    if (state.sheet) add(state.sheet.lat, state.sheet.lon);

    const sheetRequests = [];
    if (state.sheet) {
      const [x, y] = tile(state.sheet.lat, state.sheet.lon);
      for (let dx = -1; dx <= 1; dx++) for (let dy = -1; dy <= 1; dy++)
        sheetRequests.push(getJson(`${API}/cells/${Z}/${x + dx}/${y + dy}/vehicles.json`).catch(() => []));
    }
    const [arrs, fleets, vf] = await Promise.all([
      Promise.all([...cells].map(k => getJson(`${API}/cells/${Z}/${k}/arrivals.json`).catch(() => []))),
      Promise.all(state.myLines.map(l => getJson(`${API}/lines/${encodeURIComponent(l)}.json`).catch(() => []))),
      Promise.all(sheetRequests),
      loadLiveLines(),
    ]);
    state.arrivals = arrs.flat();
    state.myLines.forEach((l, i) => state.fleets.set(l, fleets[i].map(v => ({ ...v, line: l }))));
    state.sheetVehicles = vf.flat();
    state.fetchedAt = Date.now();
    state.errorMsg = '';
  } catch {
    state.errorMsg = 'Sem conexão';
  }
  render();
}

/* ---------- time ---------- */
function etaRange(a) {
  const elapsed = nowS() - a[6];
  const lo = Math.max(0, a[8] - elapsed) / 60;
  const hi = Math.max(0, a[9] - elapsed) / 60;
  if (a[5] <= 150 || hi <= 1.2) return { arriving: true };
  const l = Math.max(1, Math.floor(lo));
  return { l, h: Math.max(l + 1, Math.ceil(hi)) };
}
const rangeText = f => f.arriving ? 'chegando' : `em ${f.l} a ${f.h} min`;

function arrivalsAt(stopId, line, headsign) {
  return state.arrivals
    .filter(a => a[0] === stopId && (!line || a[1] === line) && (!headsign || a[2] === headsign))
    .sort((x, y) => (x[8] + x[9]) / 2 - (x[6] - nowS()) - ((y[8] + y[9]) / 2 - (y[6] - nowS())));
}

/* ---------- day type ----------
 * Which published service runs now: 0 weekday, 1 Saturday, 2 Sunday, or null
 * for none. Always in Rio's time zone, whatever the phone is set to, and
 * corrected for a wrong device clock. Holidays come from the GTFS calendar,
 * where the SMTR runs the Sunday service. */
const DAY_NAMES = ['dia útil', 'sábado', 'domingo'];
const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const rioClock = new Intl.DateTimeFormat('en-US', {
  timeZone: 'America/Sao_Paulo', year: 'numeric', month: '2-digit', day: '2-digit',
  // hour12 rather than hourCycle, which old WebViews ignore; some of them say
  // "24" at midnight, hence the % 24 below.
  hour: '2-digit', hour12: false, weekday: 'short',
});
let dayCache = { at: -1, v: null };

function dayType() {
  // Asked hundreds of times per render by the search; the answer changes at
  // most once a minute.
  const minute = Math.floor(nowS() / 60);
  if (dayCache.at === minute && dayCache.v) return dayCache.v;
  const p = Object.fromEntries(rioClock.formatToParts(new Date(nowS() * 1000)).map(x => [x.type, x.value]));
  const data = `${p.year}${p.month}${p.day}`;
  const weekday = WEEKDAYS.indexOf(p.weekday);
  const hour = Number(p.hour) % 24;
  const c = state.calendar;
  let v;
  if (c && data in c.exceptions) v = { svc: c.exceptions[data], holiday: true, hour };
  else v = { svc: c ? c.weekdays[weekday] : weekday === 0 ? 2 : weekday === 6 ? 1 : 0, holiday: false, hour };
  dayCache = { at: minute, v };
  return v;
}

/* Headway in minutes for this hour from a list of [svc, from, to, minutes]
   runs, or null when nothing runs now. */
function headwayFor(runs) {
  if (!runs) return null;
  const { svc, hour } = dayType();
  const f = runs.find(x => x[0] === svc && hour >= x[1] && hour < x[2]);
  return f ? f[3] : null;
}

/* Scheduled headway for this hour, from the published frequencies. */
function dirHeadwayNow(dir) {
  return dir ? headwayFor(dir.freq) : null;
}

/* ---------- left rail ---------- */
function renderRail() {
  const t = $('#rail');
  t.textContent = '';
  // Below every line, a last badge that clears them all. The rail stacks
  // upward, so the first child is the one at the bottom.
  if (state.myLines.length) {
    const x = el('button', 'line-badge clear', '×');
    x.setAttribute('aria-label', 'Remover todas as linhas');
    x.title = 'Remover todas as linhas';
    x.onclick = e => { e.stopPropagation(); removeAll(); };
    t.append(x);
  }
  for (const l of state.myLines) {
    const c = lineBadge(l, 'button');
    if (state.focused.has(l)) c.classList.add('active');
    else if (state.focused.size) c.classList.add('dimmed');
    c.setAttribute('aria-label', `Linha ${l}, opções`);
    c.setAttribute('aria-haspopup', 'menu');
    c.onclick = e => { e.stopPropagation(); openLineMenu(l, c); };
    t.append(c);
  }
}

/* ---------- line menu, as in onibus-rj ----------
 * Tapping a line's badge opens a small menu beside it: show only this line,
 * pick the direction you ride, see the times at your stop, or remove it. */
function openLineMenu(l, anchor) {
  closeMenu();
  const m = el('div', 'line-menu');
  m.id = 'menu';
  m.setAttribute('role', 'menu');
  m.style.setProperty('--color', readableColor(l));
  const item = (txt, action, extra) => {
    const b = el('button', extra || '', txt);
    b.setAttribute('role', 'menuitem');
    b.onclick = () => { closeMenu(); action(); };
    m.append(b);
    return b;
  };
  const lineState = lineStatus(l);
  if (lineState) m.append(el('div', 'menu-status' + (lineState.warning ? ' warning' : ''), lineState.txt));
  // Focus is a set: several lines can be in focus at once.
  if (state.myLines.length > 1) {
    item(state.focused.has(l) ? 'Tirar o foco desta linha' : 'Focar nesta linha', () => toggleFocus(l), 'solo');
    if (state.focused.size && !(state.focused.size === 1 && state.focused.has(l))) {
      item('Mostrar todas as linhas', () => { state.focused.clear(); render(); });
    }
  }
  const b = state.bundles.get(l);
  const current = currentDir(l);
  if (b) {
    const seen = new Set();
    for (const d of b.dirs) {
      if (seen.has(d.headsign)) continue;
      seen.add(d.headsign);
      const chosen = current && current.headsign === d.headsign;
      item(`${chosen ? '✓ ' : ''}Indo para ${d.headsign}`, () => {
        state.chosenDir[l] = d.headsign;
        saveState();
        render();
        refresh();
      }, chosen ? 'active' : '');
    }
  }
  item('Remover linha', () => follow(l, false), 'danger');

  document.body.append(m);
  const r = anchor.getBoundingClientRect();
  m.style.left = Math.min(innerWidth - m.offsetWidth - 8, r.right + 10) + 'px';
  m.style.top = '0px';
  // Measured at its final width; near the bottom it opens upward and the
  // arrow still points at the badge that was tapped.
  const h = m.offsetHeight;
  const mid = r.top + r.height / 2;
  const top = Math.max(8, Math.min(innerHeight - h - 8, mid - 22));
  m.style.top = top + 'px';
  m.style.setProperty('--arrow', `${Math.max(10, Math.min(h - 22, mid - top - 6))}px`);
  setTimeout(() => addEventListener('pointerdown', closeMenuOutside, { once: true }), 0);
}
function closeMenuOutside(e) { if (!e.target.closest('#menu')) closeMenu(); }
function closeMenu() { const m = $('#menu'); if (m) m.remove(); }

/* ---------- stop sheet ---------- */
async function openSheet(p) {
  state.sheet = { id: p.id, lat: p.lat, lon: p.lon, name: p.name, sub: p.sub };
  state.tab = 'live';
  state.callout = null;
  closeSearch();
  $('#sheet').hidden = false;
  $('#sheet').classList.remove('tall');
  document.body.classList.add('sheet-open');
  render();
  await loadStopsAround(p.lat, p.lon);
  await refresh();
  fitSheet();
}

/* The stop, and the first few buses on their way to it, above the sheet. */
function fitSheet() {
  if (!state.sheet) return;
  const pts = [[state.sheet.lat, state.sheet.lon]];
  const allBuses = [...state.sheetVehicles, ...[...state.fleets.values()].flat()];
  for (const a of arrivalsAt(state.sheet.id).slice(0, 4)) {
    const v = allBuses.find(x => x.id === a[7]);
    if (v && a[5] < 3000) pts.push([v.lat, v.lon]);
  }
  fitPoints(pts);
  drawMap();
}
function closeSheet() {
  state.sheet = null;
  state.sheetVehicles = [];
  $('#sheet').hidden = true;
  document.body.classList.remove('sheet-open');
  render();
}

function arrivalRow(group) {
  const [first, ...rest] = group.list;
  const f = etaRange(first);
  const stale = nowS() - first[6] > 90;
  const row = el('div', 'arrival');
  row.setAttribute('role', 'button');
  row.tabIndex = 0;

  const badge = listBadge(group.line, 'arrival-badge');
  const destination = el('div', 'arrival-dest', group.destination);
  const meta = el('div', 'arrival-meta');
  const starBtn = el('button', 'star-btn');
  const followed = state.myLines.includes(group.line);
  starBtn.append(icon(followed ? 'star' : 'star-empty'));
  starBtn.setAttribute('aria-label', followed ? `Parar de acompanhar ${group.line}` : `Acompanhar ${group.line}`);
  starBtn.onclick = e => { e.stopPropagation(); follow(group.line, !followed, group.destination); };
  const where = first[4] === 0 ? `a ${meters(first[5])}`
    : `${first[4] === 1 ? '1 parada' : first[4] + ' paradas'} · ${meters(first[5])}`;
  meta.append(starBtn, el('span', '', where));

  const timeEl = el('div', 'arrival-time' + (stale ? ' stale' : ''));
  const g = el('span', 'big' + (f.arriving ? ' now' : ''));
  if (f.arriving) g.append(el('b', '', 'chegando'));
  else g.append(el('b', '', `${f.l}`), el('i', '', 'a'), el('b', '', `${f.h}`), el('i', '', 'min'));
  if (!stale) g.append(icon('signal'));
  timeEl.append(g);
  if (rest.length) {
    const r = etaRange(rest[0]);
    timeEl.append(el('small', '', r.arriving ? 'depois · chegando' : `depois · ${r.l} a ${r.h} min`));
  } else {
    timeEl.append(el('small', '', stale ? `GPS ${ago(nowS() - first[6])}` : ' '));
  }

  row.append(badge, destination, timeEl, meta);
  row.onclick = () => { state.chosenDir[group.line] = group.destination; follow(group.line, true, group.destination); fitLine(group.line); render(); };
  return row;
}

async function renderSheet() {
  const box = $('#sheet-content');
  if (!state.sheet) { box.textContent = ''; return; }
  const p = state.sheet;
  box.textContent = '';

  const sheetLabel = el('div', 'sheet-label');
  sheetLabel.append(el('span', '', 'HORÁRIOS DOS ÔNIBUS'));
  const closeBtn = el('button', 'sheet-close');
  closeBtn.append(icon('close'));
  closeBtn.setAttribute('aria-label', 'Fechar');
  closeBtn.onclick = closeSheet;
  sheetLabel.append(closeBtn);
  box.append(sheetLabel);

  const head = el('div', 'station');
  const ic = el('div', 'station-icon');
  ic.append(icon('bus'));
  const names = el('div');
  names.append(el('div', 'station-name', p.name));
  const dist = distM(state.here, [p.lat, p.lon]);
  names.append(el('div', 'station-sub', [p.sub, `${walkTime(dist)} · ${meters(dist)}`].filter(Boolean).join(' · ')));
  head.append(ic, names);
  box.append(head);

  const tabs = el('div', 'segments');
  const bLive = el('button');
  bLive.append(el('span', 'live-dot'), document.createTextNode('Tempo real'));
  const bTable = el('button', '', 'Tabela de horários');
  bLive.setAttribute('aria-pressed', state.tab === 'live');
  bTable.setAttribute('aria-pressed', state.tab === 'timetable');
  bLive.onclick = () => { state.tab = 'live'; renderSheet(); };
  bTable.onclick = () => { state.tab = 'timetable'; renderSheet(); };
  tabs.append(bLive, bTable);
  box.append(tabs);

  if (state.tab === 'live') {
    const groups = new Map();
    for (const a of arrivalsAt(p.id)) {
      const k = a[1] + '\u0000' + a[2];
      if (!groups.has(k)) groups.set(k, { line: a[1], destination: a[2], list: [] });
      groups.get(k).list.push(a);
    }
    if (!groups.size) {
      box.append(el('div', 'empty', state.fetchedAt
        ? 'Nenhum ônibus a caminho desta parada agora. Veja na tabela de quanto em quanto tempo cada linha passa.'
        : 'Carregando…'));
    }
    for (const g of groups.values()) box.append(arrivalRow(g));
    box.append(el('p', 'note',
      '* Tempos estimados pela velocidade atual dos ônibus de cada linha. Podem variar com o trânsito.'));
  } else {
    await renderTimetable(box, p);
  }
}

async function renderTimetable(box, p) {
  const info = state.stops.get(p.id);
  const lines = info && info.lines ? info.lines : [...new Set(state.arrivals.filter(a => a[0] === p.id).map(a => a[1]))];
  if (!lines.length) { box.append(el('div', 'empty', 'Sem horários publicados para esta parada.')); return; }
  const list = el('div');
  box.append(list);
  const day = dayType();
  const svc = day.svc;
  const dayName = svc == null ? null : DAY_NAMES[svc];
  for (const l of lines.slice(0, 30)) {
    const b = await bundle(l);
    if (!b || state.tab !== 'timetable' || state.sheet?.id !== p.id) continue;
    for (const dir of b.dirs) {
      if (!dir.stops.some(s => s[0] === p.id)) continue;
      const block = el('div', 'freq-line');
      const topRow = el('div', 'freq-top');
      topRow.append(listBadge(l), el('b', '', dir.headsign));
      const iv = dirHeadwayNow(dir);
      topRow.append(el('span', 'freq-now', iv ? `a cada ~${iv} min` : 'sem serviço agora'));
      block.append(topRow);
      const ranges = el('div', 'freq-ranges');
      const h = day.hour;
      for (const f of dir.freq.filter(x => x[0] === svc)) {
        const s = el('span', h >= f[1] && h < f[2] ? 'current' : '', `${f[1]}h às ${f[2]}h ~${f[3]} min`);
        ranges.append(s);
      }
      if (!ranges.children.length) ranges.append(el('span', '', dayName ? `sem serviço em ${dayName}` : 'sem serviço hoje'));
      block.append(ranges);
      list.append(block);
    }
  }
  const note = !dayName ? '* A SMTR não publicou serviço para hoje.'
    : day.holiday ? `* Hoje é feriado: vale o intervalo médio publicado pela SMTR para ${dayName}. Os horários podem variar com o trânsito.`
    : `* Intervalo médio publicado pela SMTR para ${dayName}. Os horários podem variar com o trânsito.`;
  box.append(el('p', 'note', note));
}

function wireSheet() {
  const getJson = $('#grabber');
  const f = $('#sheet');
  let drag = null;
  getJson.addEventListener('pointerdown', e => {
    drag = { y: e.clientY, h: f.getBoundingClientRect().height, mov: 0 };
    getJson.setPointerCapture(e.pointerId);
    f.classList.add('dragging');
  });
  getJson.addEventListener('pointermove', e => {
    if (!drag) return;
    drag.mov = e.clientY - drag.y;
    f.style.height = Math.max(120, drag.h - drag.mov) + 'px';
  });
  getJson.addEventListener('pointerup', () => {
    if (!drag) return;
    f.classList.remove('dragging');
    f.style.height = '';
    if (drag.mov > 120) closeSheet();
    else if (drag.mov < -40) f.classList.add('tall');
    else if (drag.mov > 40) f.classList.remove('tall');
    else if (Math.abs(drag.mov) < 6) f.classList.toggle('tall');
    drag = null;
  });
}

/* ---------- search ---------- */
function openSearch() {
  state.searching = true;
  document.body.classList.add('searching');
  if (!state.freq) loadFreq().then(() => { if (state.searching) renderSuggestions(); });
  loadLiveLines().then(() => { if (state.searching) renderSuggestions(); });
  state.callout = null;
  $('#suggestions').hidden = false;
  const ic = $('#bar-icon');
  ic.textContent = '';
  ic.append(icon('back'));
  ic.setAttribute('aria-label', 'Fechar busca');
  renderSuggestions();
}
function closeSearch() {
  if (!state.searching) return;
  state.searching = false;
  document.body.classList.remove('searching');
  state.query = '';
  $('#search-field').value = '';
  $('#search-field').blur();
  $('#suggestions').hidden = true;
  const ic = $('#bar-icon');
  ic.textContent = '';
  ic.append(icon('bus'));
  ic.setAttribute('aria-label', 'Buscar linha');
}

function routeName(c) {
  // "Irajá - Castelo" reads as "Irajá x Castelo", the way the incumbent lists lines.
  return (c[1] || c[3].join(' - ')).replace(/\s+-\s+/g, ' x ');
}

function suggestionRow(name, caption, fixedTitle) {
  const c = state.catalog.find(x => x[0] === name);
  const row = el('div', 'suggestion');
  row.setAttribute('role', 'button');
  row.tabIndex = 0;
  const followed = state.myLines.includes(name);
  const e = el('button', 'suggestion-star');
  e.append(icon(followed ? 'star' : 'star-empty'));
  e.setAttribute('aria-label', followed ? `Parar de acompanhar ${name}` : `Acompanhar ${name}`);
  e.onclick = ev => { ev.stopPropagation(); follow(name, !followed); renderSuggestions(); };
  const badge = listBadge(name);
  const t = el('div', 'suggestion-text');
  const title = el('div');
  const txt = fixedTitle || (c ? routeName(c) : name);
  const q = state.query.trim();
  const i = q ? txt.toLowerCase().indexOf(q.toLowerCase()) : -1;
  if (i >= 0) {
    title.append(txt.slice(0, i), el('b', '', txt.slice(i, i + q.length)), txt.slice(i + q.length));
  } else title.textContent = txt;
  t.append(title);
  // A caption is plain text, or a line status that may carry a warning.
  if (caption) t.append(el('small', caption.warning ? 'warning' : '', caption.txt ?? caption));
  row.append(e, badge, t);
  row.onclick = async () => { closeSearch(); await follow(name, true); fitLine(name); render(); };
  return row;
}

async function loadFreq() {
  if (state.freq) return state.freq;
  state.freq = await getJson(`${STATIC}/freq.json`).catch(() => ({}));
  return state.freq;
}

/* Scheduled headway of a line for this hour, in minutes, or null when it does
   not run now. */
function headwayNow(l) {
  return headwayFor(state.freq && state.freq[l]);
}

const busesText = n => n === 1 ? '1 ônibus' : `${n} ônibus`;

/* Where a line stands right now, in one short sentence, or null while the
   live counts have not arrived. Picking a line must never leave the screen
   silent: no bus reporting, not running at this hour, and no published route
   all say so. */
function lineStatus(l) {
  const c = state.catalog.find(x => x[0] === l);
  const live = state.liveLines.get(l);
  const n = live ? live[0] : 0;
  if (!c) {
    if (n) return { txt: `Rota não publicada pela SMTR · ${busesText(n)} agora` };
    return state.liveLinesAt ? { txt: 'Nenhum ônibus desta linha nos dados da SMTR agora', warning: true } : null;
  }
  const h = headwayNow(l);
  const every = h ? ` · a cada ~${h} min` : '';
  if (n) return { txt: `${busesText(n)} agora${every}` };
  if (!state.liveLinesAt) return h ? { txt: `A cada ~${h} min` } : null;
  const f = state.freq && state.freq[l];
  if (f && h == null) {
    const { svc, hour } = dayType();
    const today = f.filter(x => x[0] === svc);
    const nextHour = today.map(x => x[1]).filter(x => x > hour).sort((a, b) => a - b)[0];
    if (nextHour != null) return { txt: `Não opera neste horário · volta às ${nextHour} h`, warning: true };
    return { txt: today.length ? 'Não opera mais hoje' : 'Não opera hoje', warning: true };
  }
  return { txt: `Nenhum ônibus com sinal agora${every}`, warning: true };
}

/* Codes that differ from q by at most two edits, closest first, for a search
   that found nothing: "2335" typed as "2353", "SV744" for "SV774". */
function similarCodes(q) {
  const lev = (a, b) => {
    const d = Array.from({ length: b.length + 1 }, (_, i) => i);
    for (let i = 1; i <= a.length; i++) {
      let prev = d[0];
      d[0] = i;
      for (let j = 1; j <= b.length; j++) {
        const t = d[j];
        d[j] = Math.min(d[j] + 1, d[j - 1] + 1, prev + (a[i - 1] === b[j - 1] ? 0 : 1));
        prev = t;
      }
    }
    return d[b.length];
  };
  const tapTarget = q.toUpperCase();
  const codes = new Set(state.catalog.map(c => c[0]));
  for (const [l, [, route]] of state.liveLines) if (!route) codes.add(l);
  return [...codes]
    .map(l => ({ l, d: lev(tapTarget, l.toUpperCase()) }))
    .filter(x => x.d <= 2)
    .sort((a, b) => a.d - b.d || a.l.length - b.l.length)
    .slice(0, 3)
    .map(x => x.l);
}

/* Lines near you, best first. "Best" is the time until you are on board: the
   walk to the stop (4.5 km/h) plus the average wait, half the headway. A line
   every 3 min at 400 m beats one every 20 min at your door, and a frequent
   line two kilometres away does not appear at all. */
function nearbyLines() {
  const m = new Map();
  for (const s of state.stops.values()) {
    if (!s.lines) continue;
    const d = distM(state.here, [s.lat, s.lon]);
    if (d > NEARBY_RADIUS) continue;
    for (const l of s.lines) if (!m.has(l) || d < m.get(l)) m.set(l, d);
  }
  return [...m].map(([l, d]) => {
    const h = headwayNow(l);
    return { l, d, h, cost: d / 75 + (h == null ? 1e3 : h / 2) };
  }).sort((a, b) => a.cost - b.cost);
}

function renderSuggestions() {
  const box = $('#suggestions');
  if (!state.searching) return;
  box.textContent = '';
  const q = state.query.trim().toLowerCase();
  if (q) {
    const hits = state.catalog.filter(c =>
      c[0].toLowerCase().startsWith(q) || c[1].toLowerCase().includes(q) ||
      c[3].some(d => d.toLowerCase().includes(q)) ||
      (c[4] || []).some(a => a.toLowerCase().startsWith(q))).slice(0, 30);
    // Lines on the road whose route the SMTR has not published yet.
    const noRoute = [...state.liveLines].filter(([l, [, route]]) => !route && l.toLowerCase().startsWith(q));
    for (const c of hits) box.append(suggestionRow(c[0], lineStatus(c[0])));
    for (const [l, [n]] of noRoute) box.append(suggestionRow(l, `${busesText(n)} agora`, 'Rota não publicada pela SMTR'));
    if (!hits.length && !noRoute.length) {
      box.append(el('div', 'suggestion-empty', `Nenhuma linha com "${state.query}".`));
      const p = similarCodes(state.query.trim());
      if (p.length) {
        box.append(el('div', 'suggestion-group', 'Talvez você procure'));
        for (const l of p) box.append(suggestionRow(l, lineStatus(l)));
      }
      box.append(el('p', 'suggestion-note', 'O busones mostra as linhas municipais do Rio e o BRT. Linhas intermunicipais e de outros municípios não fazem parte dos dados da SMTR.'));
    }
    return;
  }
  if (state.myLines.length) {
    box.append(el('div', 'suggestion-group', 'Suas linhas'));
    for (const l of state.myLines) box.append(suggestionRow(l, lineStatus(l)));
  }
  const nearby = nearbyLines().filter(x => !state.myLines.includes(x.l));
  box.append(el('div', 'suggestion-group', nearby.length ? 'Perto de você' : 'Nenhuma linha perto de você'));
  for (const x of nearby.slice(0, 25)) {
    const when = x.h == null ? 'sem ônibus nesta hora' : `a cada ~${x.h} min`;
    box.append(suggestionRow(x.l, `${when} · ${walkTime(x.d)}`));
  }
}

/* ---------- following ---------- */
async function follow(l, yes, headsign) {
  if (yes && !state.myLines.includes(l)) {
    if (state.myLines.length >= 8) state.myLines.shift();
    state.myLines.push(l);
    if (headsign) state.chosenDir[l] = headsign;
    saveState();
    await bundle(l);
    render();
    await refresh();
  } else if (!yes) {
    state.myLines = state.myLines.filter(x => x !== l);
    state.fleets.delete(l);
    state.focused.delete(l);
    saveState();
    render();
  } else if (headsign) {
    state.chosenDir[l] = headsign;
    saveState();
    render();
  }
}

/* Removes every line at once. */
function removeAll() {
  closeMenu();
  if (!state.myLines.length) return;
  state.myLines = [];
  state.focused.clear();
  state.fleets.clear();
  state.callout = null;
  saveState();
  render();
}

/* Adds a line to the focus, or takes it out. Focusing frames the line. */
function toggleFocus(l) {
  if (state.focused.has(l)) state.focused.delete(l);
  else { state.focused.add(l); fitLine(l); }
  render();
}

/* Frame you, the stop you would use, and the buses on their way to it. */
function fitLine(l) {
  const d = currentDir(l);
  if (!d) {
    // A line without a route: you and its nearest buses.
    const nearby = (state.fleets.get(l) || []).map(v => [v.lat, v.lon])
      .sort((a, b) => distM(state.here, a) - distM(state.here, b)).slice(0, 3);
    if (nearby.length) fitPoints([state.here, ...nearby]);
    return;
  }
  const p = nearestStop(d);
  const pts = [state.here];
  if (p) pts.push([p.lat, p.lon]);
  if (p) for (const a of arrivalsAt(p.id, l, d.headsign).slice(0, 2)) {
    const v = (state.fleets.get(l) || []).find(x => x.id === a[7]);
    if (v) pts.push([v.lat, v.lon]);
  }
  fitPoints(pts);
}

/* ---------- bus callout ---------- */
function renderCallout() {
  const b = $('#callout');
  if (!state.callout) { b.hidden = true; return; }
  const v = state.callout;
  const pos = toScreen(v.lat, v.lon);
  b.hidden = false;
  b.textContent = '';
  const bodyEl = el('div', 'callout-body');
  const lb = el('div', 'callout-line');
  lb.style.setProperty('--color', lineColor(v.line));
  lb.style.setProperty('--color-text', lineTextColor(v.line));
  lb.append(el('b', '', v.line), el('small', '', v.id));
  const info = el('div', 'callout-info');
  const s1 = el('div', 'signal');
  s1.append(icon('signal'), document.createTextNode(ago(nowS() - v.t)));
  info.append(s1, el('div', '', `${Math.round(v.spd || 0)} km/h`));
  bodyEl.append(lb, info);
  b.append(bodyEl);
  const footer = el('div', 'callout-footer');
  const noRoute = state.bundles.has(v.line) && !state.bundles.get(v.line);
  footer.append(document.createTextNode(v.dir ? `Sentido ${v.to || v.dir}`
    : noRoute ? 'Rota não publicada pela SMTR' : 'Sentido ainda não confirmado'));
  const a = state.arrivals.find(x => x[7] === v.id && (!state.sheet || x[0] === state.sheet.id));
  if (a) {
    const f = etaRange(a);
    footer.append(document.createTextNode(' · '), el('b', '', f.arriving ? 'chegando' : `chega ${rangeText(f)}`));
  }
  b.append(footer);

  // Keep the whole callout on screen: shift it sideways near an edge and open
  // it below the bus near the top, with the arrow still on the bus.
  const w = b.offsetWidth, h = b.offsetHeight;
  const left = Math.max(8, Math.min(innerWidth - w - 8, pos[0] - w / 2));
  const below = pos[1] - h - 22 < 76;
  b.style.left = left + 'px';
  b.style.top = (below ? pos[1] + 22 : pos[1] - h - 22) + 'px';
  b.style.setProperty('--arrow-x', `${Math.max(14, Math.min(w - 14, pos[0] - left))}px`);
  b.classList.toggle('below', below);
}


/* Called by the map when a bus or a stop is tapped. */
function onMapTap(tapTarget) {
  closeMenu();
  if (state.searching) { closeSearch(); return; }
  if (!tapTarget) { if (state.callout) { state.callout = null; renderCallout(); } return; }
  if (tapTarget.kind === 'bus') {
    const a = state.arrivals.find(x => x[7] === tapTarget.v.id);
    state.callout = { ...tapTarget.v, color: tapTarget.color, to: a ? a[2] : null };
    renderCallout();
  } else if (tapTarget.kind === 'stop') {
    openSheet(tapTarget.p);
  }
}

/* ---------- settings ---------- */
function openSettings() {
  const box = $('#settings-list');
  box.textContent = '';
  const items = [
    ['dark', 'Modo escuro', ''],
    ['route', 'Exibir rota', 'O traçado das linhas que você acompanha'],
    ['stops', 'Exibir paradas', 'Pontos de ônibus no mapa'],
    ['staleBuses', 'Exibir ônibus sem sinal', 'Ônibus que não enviam posição há mais de 3 minutos'],
  ];
  for (const [k, title, sub] of items) {
    const b = el('button', 'setting');
    b.setAttribute('role', 'switch');
    b.setAttribute('aria-checked', !!state.settings[k]);
    const t = el('div', 'setting-text', title);
    if (sub) t.append(el('small', '', sub));
    b.append(t, el('span', 'toggle'));
    b.onclick = () => {
      state.settings[k] = !state.settings[k];
      saveState();
      applyTheme();
      openSettings();
      render();
    };
    box.append(b);
  }
  $('#settings').hidden = false;
}
function applyTheme() {
  document.documentElement.dataset.theme = state.settings.dark ? 'dark' : 'light';
  document.querySelector('meta[name=theme-color]').content = state.settings.dark ? '#202124' : '#ffffff';
}

/* ---------- map scene ---------- */
/* What the map shows is decided by what you are doing, not by what is known.
 * By default: the route of each line you follow, only in the direction you
 * ride; the stop you would walk to; and the next few buses on their way to
 * it. Everything else waits for a reason to appear: focusing a line shows all
 * of its buses and its stops, opening a stop shows the buses coming to it,
 * and zooming in shows the stops around you. */
const MAX_PER_LINE = 3;

function busesComingToMe(l) {
  const d = currentDir(l);
  const p = d && nearestStop(d);
  if (!p) return [];
  const fleet = state.fleets.get(l) || [];
  return arrivalsAt(p.id, l, d.headsign)
    .slice(0, MAX_PER_LINE)
    .map(a => fleet.find(v => v.id === a[7]))
    .filter(Boolean);
}

function scene() {
  const c = { routes: [], yours: [], dots: [], others: [], buses: [], highlight: null };
  const staleOk = state.settings.staleBuses;
  const focusSet = state.focused;
  const seen = new Set();
  const addBus = (v, color) => {
    if (seen.has(v.id) || v.ph === 'parked') return;
    const stale = nowS() - v.t > 180;
    if (stale && !staleOk) return;
    seen.add(v.id);
    c.buses.push({ v, color, stale, numberTag: null });
  };

  for (const l of state.myLines) {
    if (focusSet.size && !focusSet.has(l)) continue;
    const b = state.bundles.get(l);
    const d = currentDir(l);
    const color = mapColor(l);
    if (!b) {
      // No published route: no direction to pick and no stop to wait at, so
      // every bus of the line is shown.
      for (const v of state.fleets.get(l) || []) addBus(v, color);
      continue;
    }
    if (!d) continue;
    if (state.settings.route) c.routes.push({ color, pts: d.pts, strong: focusSet.has(l) });
    const p = nearestStop(d);
    if (p && !state.sheet) {
      // Several lines can share the stop you would use: one sign, one label.
      const existing = c.yours.find(s => s.id === p.id);
      if (existing) existing.lines.push(l);
      else c.yours.push({ ...p, color, lines: [l] });
    }
    // The stops of your lines only, never every stop in the area: with lines
    // in focus, only theirs. Small dots in the line's colour, from zoom 15.
    if (state.settings.stops && view.z >= 15 && !state.sheet) {
      for (const s of d.stops) {
        if (p && s[0] === p.id) continue;
        c.dots.push({ lat: s[1], lon: s[2], color, id: s[0], name: s[3], sub: s[5] || '' });
      }
    }
    if (focusSet.has(l)) {
      // In focus: every bus riding this direction, not just the next few.
      for (const v of state.fleets.get(l) || []) if (v.shp === d.shape) addBus(v, color);
    } else if (!state.sheet) {
      for (const v of busesComingToMe(l)) addBus(v, color);
    }
  }

  if (state.sheet) {
    // A stop is open: it, and the buses on their way to it.
    c.highlight = state.sheet;
    const fleets = [...state.fleets.values()].flat();
    for (const a of arrivalsAt(state.sheet.id).slice(0, 8)) {
      const v = fleets.find(x => x.id === a[7]) || state.sheetVehicles.find(x => x.id === a[7]);
      if (v) addBus({ ...v, line: a[1] }, mapColor(a[1]));
    }
  }
  // Lines of one region share its colour. When two lines on the map do, their
  // buses carry the number, the way apps in cities where every bus is red
  // tell them apart.
  const linesByColor = new Map();
  for (const o of c.buses) {
    if (!linesByColor.has(o.color)) linesByColor.set(o.color, new Set());
    linesByColor.get(o.color).add(o.v.line);
  }
  for (const o of c.buses) {
    if (linesByColor.get(o.color).size > 1) {
      o.numberTag = { txt: o.v.line, bg: lineColor(o.v.line), text: lineTextColor(o.v.line) };
    }
  }
  return c;
}

/* ---------- render ---------- */
function render() {
  try {
    renderRail();
    if (state.sheet) renderSheet();
    if (state.searching) renderSuggestions();
  } catch (e) {
    console.error(e);
  }
  drawMap();
  renderCallout();
}

/* ---------- where you are ----------
 * GPS, a place picked on the map, or a neighbourhood or stop found by name.
 * A place you chose is remembered, as onibus-rj did, so opening the app at the
 * same stop tomorrow needs no GPS at all. Names come from our own data (1,011
 * neighbourhoods, 7,694 stops), so there is no geocoding service to call. */
const norm = s => String(s).normalize('NFD').replace(/[\u0300-\u036f]/g, '').toLowerCase();

function savePlace() {
  try {
    localStorage.setItem('busones:place', JSON.stringify(
      state.place.mode === 'manual' ? { mode: 'manual', lat: state.here[0], lon: state.here[1], name: state.place.name } : { mode: state.place.mode }));
  } catch {}
}
function savedPlace() {
  try { return JSON.parse(localStorage.getItem('busones:place') || 'null'); } catch { return null; }
}

function updateWhere() {
  const t = $('#where-text');
  if (!t) return;
  t.textContent = state.place.mode === 'manual' ? state.place.name
    : state.place.mode === 'gps' ? 'Minha localização' : 'Onde você está?';
}

async function setPlace(lat, lon, name, mode, opts = {}) {
  state.here = [lat, lon];
  state.hasPlace = true;
  state.place = { mode, name };
  savePlace();
  updateWhere();
  if (opts.recenter !== false) {
    view.center = [lat, lon];
    view.z = Math.max(view.z, 15);
  }
  render();
  if (opts.address) addressName(lat, lon);
  await loadStopsAround(lat, lon);
  render();
  await refresh();
}

/* The street and number of a point, as onibus-rj showed it; the local name
   (nearest stop or neighbourhood) stays until the answer arrives. */
let reverseSeq = 0;
async function addressName(lat, lon) {
  const seq = ++reverseSeq;
  try {
    const r = await fetch(`${GEO}/reverse?lat=${lat.toFixed(5)}&lon=${lon.toFixed(5)}`);
    if (!r.ok) return;
    const l = await r.json();
    if (!l || !l.name || seq !== reverseSeq || state.place.mode !== 'manual') return;
    if (Math.abs(state.here[0] - lat) > 1e-6 || Math.abs(state.here[1] - lon) > 1e-6) return;
    state.place.name = l.name;
    savePlace();
    updateWhere();
  } catch {}
}

/* Called by the map: the pin was dragged, or a point was held down. */
function useGps(quiet) {
  if (!navigator.geolocation) { if (!quiet) placeNotice('Este navegador não informa a localização.'); return; }
  navigator.geolocation.getCurrentPosition(
    p => { closePlacePanel(); setPlace(p.coords.latitude, p.coords.longitude, 'Minha localização', 'gps'); },
    () => {
      if (quiet && !state.hasPlace) openPlacePanel('Não conseguimos sua localização. Escolha onde você está.');
      else if (!quiet) placeNotice('Não foi possível usar o GPS. Busque um endereço ou local, ou escolha no mapa.');
    },
    { enableHighAccuracy: true, timeout: 8000, maximumAge: 30000 });
}

function nearbyName(lat, lon) {
  let stop = null, dp = Infinity;
  for (const s of state.stops.values()) {
    const d = distM([lat, lon], [s.lat, s.lon]);
    if (d < dp) { dp = d; stop = s; }
  }
  if (stop && dp <= 250) return stop.name;
  let neighborhood = null, db = Infinity;
  for (const p of placeNames || []) {
    const d = distM([lat, lon], [p[1], p[0]]);
    if (d < db) { db = d; neighborhood = p[2]; }
  }
  return neighborhood && db < 3000 ? neighborhood : 'Local escolhido';
}

async function loadAllStops() {
  if (state.allStops) return state.allStops;
  state.allStops = await getJson(`${STATIC}/stops.json`).catch(() => []);
  for (const s of state.allStops) s.n = norm(s[3]);
  return state.allStops;
}

function placeRow(iconName, cls, title, sub, action, current) {
  const b = el('button', 'place-row' + (current ? ' current' : ''));
  const ic = el('span', 'place-row-icon ' + cls);
  ic.append(icon(iconName));
  const t = el('span', 'place-row-text');
  t.append(el('b', '', title));
  if (sub) t.append(el('small', '', sub));
  b.append(ic, t);
  b.onclick = action;
  return b;
}

let placeSearchSeq = 0;
async function renderPlacePanel(warning) {
  const list = $('#place-list');
  const q = norm($('#place-field').value.trim());
  const seq = ++placeSearchSeq;
  list.textContent = '';
  if (warning) list.append(el('p', 'place-notice', warning));
  if (!q) {
    list.append(
      placeRow('crosshair', 'gps', 'Usar minha localização', 'Pelo GPS do aparelho', () => useGps(false), state.place.mode === 'gps'),
      placeRow('pin', 'map', 'Escolher no mapa', 'Mova o mapa até o ponto certo', startPick, false));
    return;
  }
  loadPlaceNames();
  // Full addresses come from the backend on request, never per keystroke:
  // Nominatim's usage policy forbids search-as-you-type.
  if (q.length >= 3) {
    const raw = $('#place-field').value.trim();
    list.append(placeRow('search', 'gps', `Buscar endereço "${raw}"`, 'Rua e número, ou um lugar',
      () => searchAddress(raw), false));
  }
  // One entry per name: two stops called "Metrô Glória" are the two sides of
  // the same street, and either is a fine answer to "where are you".
  const unique = (list, name) => {
    const seen = new Set();
    return list.filter(x => { const k = norm(name(x)); if (seen.has(k)) return false; seen.add(k); return true; });
  };
  const neighborhoods = unique((placeNames || [])
    .filter(p => norm(p[2]).includes(q))
    .sort((x, y) => (norm(x[2]).startsWith(q) ? 0 : 1) - (norm(y[2]).startsWith(q) ? 0 : 1) || x[3] - y[3]), p => p[2])
    .slice(0, 6);
  for (const p of neighborhoods) {
    list.append(placeRow('pin', '', p[2], p[3] === 2 ? 'Bairro' : 'Região',
      () => { closePlacePanel(); setPlace(p[1], p[0], p[2], 'manual'); }, false));
  }
  const stops = await loadAllStops();
  if (seq !== placeSearchSeq) return;
  const found = unique(stops.filter(s => s.n.includes(q))
    .sort((x, y) => (x.n.startsWith(q) ? 0 : 1) - (y.n.startsWith(q) ? 0 : 1)), s => s[3]).slice(0, 12);
  for (const s of found) {
    list.append(placeRow('stop', 'stop', s[3], s[4] ? `Parada · ${s[4]}` : 'Parada',
      () => { closePlacePanel(); setPlace(s[1], s[2], s[3], 'manual'); }, false));
  }
  // With nothing from our own data, the address search has not run yet: point
  // at it instead of saying nothing was found.
  if (!neighborhoods.length && !found.length) {
    list.append(el('p', 'place-notice', q.length >= 3
      ? 'Nenhum bairro ou parada com esse nome. Toque em "Buscar endereço" ou aperte Enter para procurar a rua.'
      : 'Digite pelo menos 3 letras para buscar um endereço.'));
  }
}

async function searchAddress(text) {
  const list = $('#place-list');
  const seq = ++placeSearchSeq;
  list.textContent = '';
  list.append(el('p', 'place-notice', 'Buscando endereço…'));
  try {
    const r = await fetch(`${GEO}/geocode?q=${encodeURIComponent(text)}`);
    if (seq !== placeSearchSeq) return;
    const foundPlaces = r.ok ? await r.json() : null;
    list.textContent = '';
    if (!foundPlaces) {
      list.append(el('p', 'place-notice', 'A busca de endereço não respondeu agora. Tente bairro ou parada, ou escolha no mapa.'));
      return;
    }
    if (!foundPlaces.length) {
      list.append(el('p', 'place-notice', `Nenhum endereço encontrado para "${text}" no Rio.`));
      return;
    }
    // When the map has no such house number, the result is the street: say
    // so, since the pin will land somewhere along it, not at the door. A house
    // number comes first or last ("304 Bornéo", "Bornéo, 304"); a number
    // followed by "de" is part of the name ("7 de Setembro").
    const m = text.match(/^\s*(\d{1,5})\s+(?!de\b)/i) || text.match(/[\s,]+(\d{1,5})\s*$/);
    const number = m && m[1];
    for (const l of foundPlaces) {
      const noNumber = number && !new RegExp(`\\b${number}\\b`).test(l.name);
      const sub = [l.area || 'Endereço', noNumber ? `número ${number} não encontrado` : ''].filter(Boolean).join(' · ');
      list.append(placeRow('pin', '', l.name, sub,
        () => { closePlacePanel(); setPlace(l.lat, l.lon, l.name, 'manual'); }, false));
    }
  } catch {
    if (seq === placeSearchSeq) {
      list.textContent = '';
      list.append(el('p', 'place-notice', 'Sem conexão com a busca de endereço.'));
    }
  }
}

function openPlacePanel(warning) {
  closeSearch();
  closeMenu();
  state.callout = null;
  $('#place-field').value = '';
  $('#place-panel').hidden = false;
  document.body.classList.add('place-open');
  loadPlaceNames();
  renderPlacePanel(warning);
}
function closePlacePanel() { $('#place-panel').hidden = true; document.body.classList.remove('place-open'); }
function placeNotice(txt) { if ($('#place-panel').hidden) openPlacePanel(txt); else renderPlacePanel(txt); }

function startPick() {
  closePlacePanel();
  closeSheet();
  state.pickingPlace = true;
  document.body.classList.add('picking');
  $('#pick-bar').hidden = false;
  if (view.z < 15) view.z = 15;
  render();
}
function endPick() {
  state.pickingPlace = false;
  document.body.classList.remove('picking');
  $('#pick-bar').hidden = true;
  render();
}
async function confirmPick() {
  const [lat, lon] = view.center;
  endPick();
  await loadStopsAround(lat, lon);
  setPlace(lat, lon, nearbyName(lat, lon), 'manual', { address: true });
}

/* ---------- boot ---------- */
async function init() {
  loadState();
  const q = new URLSearchParams(location.search);
  const saved = savedPlace();
  if (q.get('at')) {
    const [la, lo] = q.get('at').split(',').map(Number);
    if (isFinite(la) && isFinite(lo)) { state.here = [la, lo]; state.hasPlace = true; state.place = { mode: 'manual', name: 'Local escolhido' }; }
  } else if (saved && saved.mode === 'manual' && isFinite(saved.lat)) {
    state.here = [saved.lat, saved.lon];
    state.hasPlace = true;
    state.place = { mode: 'manual', name: saved.name || 'Local escolhido' };
  }
  if (q.get('lines')) { state.myLines = q.get('lines').split(',').filter(Boolean).slice(0, 8); saveState(); }
  applyTheme();
  view.center = [...state.here];
  view.z = 15;
  wireSheet();

  const field = $('#search-field');
  field.addEventListener('focus', openSearch);
  field.addEventListener('input', () => { state.query = field.value; renderSuggestions(); });
  field.addEventListener('keydown', e => { if (e.key === 'Escape') closeSearch(); });
  $('#bar-icon').onclick = () => state.searching ? closeSearch() : field.focus();
  $('#search-btn').onclick = () => field.focus();
  $('#fab-locate').onclick = () => useGps(false);
  $('#where').onclick = () => openPlacePanel();
  $('#place-close').onclick = closePlacePanel;
  $('#place-field').addEventListener('input', () => renderPlacePanel());
  $('#place-field').addEventListener('keydown', e => {
    const v = $('#place-field').value.trim();
    if (e.key === 'Enter' && v.length >= 3) searchAddress(v);
  });
  $('#pick-cancel').onclick = endPick;
  $('#pick-ok').onclick = confirmPick;
  $('#gear').onclick = openSettings;
  $('#settings-back').onclick = () => { $('#settings').hidden = true; };

  [state.catalog, state.calendar] = await Promise.all([
    getJson(`${STATIC}/lines.json`).catch(() => []),
    getJson(`${STATIC}/calendar.json`).catch(() => null),
  ]);
  dayCache.at = -1;
  state.colors = new Map(state.catalog.map(c => [c[0], [c[5] || '', c[6] || '']]));
  await loadStopsAround(state.here[0], state.here[1]);
  await Promise.all(state.myLines.map(bundle));
  render();
  await refresh();
  if (q.get('stop')) {
    const p = state.stops.get(q.get('stop'));
    if (p) openSheet(p);
  }
  if (q.get('search') != null) {
    state.query = q.get('search');
    $('#search-field').value = state.query;
    openSearch();
  }
  updateWhere();
  if (q.get('pick')) startPick();
  else if (!q.get('at') && state.place.mode !== 'manual') useGps(true);
  setInterval(refresh, REFRESH_MS);
  setInterval(renderCallout, 5000);
}
