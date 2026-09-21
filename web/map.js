/* A small slippy map on one canvas, drawn the way Lá vem o ônibus draws its
 * map: navy stop signs, bus icons in the colour of their line, routes in the
 * same colour. Two things differ on purpose, both from the incumbent's own
 * reviews: markers grow with the zoom ("tudo aumenta de tamanho, menos os
 * ônibus"), and each bus carries a pointer showing which way it is going. */

const BUS = new Path2D('M4 16c0 .88.39 1.67 1 2.22V20c0 .55.45 1 1 1h1c.55 0 1-.45 1-1v-1h8v1c0 .55.45 1 1 1h1c.55 0 1-.45 1-1v-1.78c.61-.55 1-1.34 1-2.22V6c0-3.5-3.58-4-8-4s-8 .5-8 4v10zm3.5 1c-.83 0-1.5-.67-1.5-1.5S6.67 14 7.5 14s1.5.67 1.5 1.5S8.33 17 7.5 17zm9 0c-.83 0-1.5-.67-1.5-1.5s.67-1.5 1.5-1.5 1.5.67 1.5 1.5-.67 1.5-1.5 1.5zm1.5-6H6V6h12v5z');

const view = { center: [-22.9068, -43.1729], z: 15, W: 0, H: 0, dpr: 1 };
let targets = [];

const cvs = () => document.getElementById('map');
const world = z => 256 * 2 ** z;
function proj(lat, lon, z) {
  const s = world(z), sin = Math.sin(lat * Math.PI / 180);
  return [(lon + 180) / 360 * s, (0.5 - Math.log((1 + sin) / (1 - sin)) / (4 * Math.PI)) * s];
}
function unproject(x, y, z) {
  const s = world(z), n = Math.PI - 2 * Math.PI * y / s;
  return [180 / Math.PI * Math.atan(0.5 * (Math.exp(n) - Math.exp(-n))), x / s * 360 - 180];
}
function origin() {
  const [cx, cy] = proj(view.center[0], view.center[1], view.z);
  return [cx - view.W / 2, cy - view.H / 2];
}
function toScreen(lat, lon) {
  const [ox, oy] = origin(), [x, y] = proj(lat, lon, view.z);
  return [x - ox, y - oy];
}

/* ---------- our basemap ----------
 * Water, green areas, beaches, streets and names, drawn from small vector
 * tiles cut from OpenStreetMap by `busones base build`. Each tile's layers are
 * turned into Path2D objects once, in tile coordinates, and redrawn with a
 * transform, so panning costs a few fills per tile. */
const BASE = '/dist/base';
/* You on the map, in the app's blue: a dot from GPS, a pin you placed. */
const HERE_COLOR = '#1a73e8';

const BASE_COLORS = {
  light: { land: '#f4f4f1', water: '#c9e3f0', green: '#dcebd3', sand: '#f3ecd8',
           road: '#ffffff', roadCasing: '#e1e2e4', motorway: '#fbe7ad', motorwayCasing: '#ead08b',
           placeHit: '#6f747a', street: '#80858b', halo: 'rgba(244,244,241,.92)' },
  dark: { land: '#1c1d20', water: '#152736', green: '#1d2922', sand: '#2a2822',
            road: '#34363b', roadCasing: '#1c1d20', motorway: '#4a4231', motorwayCasing: '#1c1d20',
            placeHit: '#8f959b', street: '#8a9096', halo: 'rgba(28,29,32,.92)' },
};
const baseTiles = new Map();
let placeNames = null;

function sourceZoom(z) { return z < 12.5 ? 11 : z < 14.5 ? 13 : 15; }

function path(polygons) {
  const p = new Path2D();
  for (const rings of polygons) for (const r of rings) {
    p.moveTo(r[0], r[1]);
    for (let i = 2; i < r.length; i += 2) p.lineTo(r[i], r[i + 1]);
    p.closePath();
  }
  return p;
}
function lines(ls) {
  const p = new Path2D();
  for (const l of ls) {
    p.moveTo(l[0], l[1]);
    for (let i = 2; i < l.length; i += 2) p.lineTo(l[i], l[i + 1]);
  }
  return p;
}

function baseTile(z, x, y) {
  const k = `${z}/${x}/${y}`;
  let t = baseTiles.get(k);
  if (t) return t;
  t = { ready: false };
  baseTiles.set(k, t);
  fetch(`${BASE}/${k}.json`).then(r => r.ok ? r.json() : null).then(d => {
    if (d) {
      t.water = d.w ? path(d.w) : null;
      t.green = d.g ? path(d.g) : null;
      t.sand = d.s ? path(d.s) : null;
      t.r0 = d.r0 ? lines(d.r0) : null;
      t.r1 = d.r1 ? lines(d.r1) : null;
      t.r2 = d.r2 ? lines(d.r2) : null;
      t.names = d.l || [];
    }
    t.ready = true;
    request();
  }).catch(() => { t.ready = true; });
  if (baseTiles.size > 500) baseTiles.delete(baseTiles.keys().next().value);
  return t;
}

function loadPlaceNames() {
  if (placeNames) return;
  placeNames = [];
  fetch(`${BASE}/places.json`).then(r => r.json()).then(d => { placeNames = d; request(); }).catch(() => {});
}

/* Road widths in screen pixels, growing with the zoom like any street map. */
function widths(z) {
  const f = Math.max(0, z - 13);
  return { motorway: 2.2 + f * 1.6, major: 1.6 + f * 1.4, place: z >= 15 ? 0.8 + (z - 15) * 1.3 : 0 };
}

function drawBase(ctx) {
  const color = BASE_COLORS[dark() ? 'dark' : 'light'];
  ctx.fillStyle = color.land;
  ctx.fillRect(0, 0, view.W, view.H);
  const sz = sourceZoom(view.z);
  const n = 2 ** sz;
  const side = 256 * 2 ** (view.z - sz);
  const k = side / 1024;
  const [ox, oy] = origin();
  const vis = [];
  for (let x = Math.floor(ox / side); x <= Math.floor((ox + view.W) / side); x++) {
    for (let y = Math.floor(oy / side); y <= Math.floor((oy + view.H) / side); y++) {
      if (y < 0 || y >= n || x < 0 || x >= n) continue;
      const t = baseTile(sz, x, y);
      if (t.ready) vis.push({ t, x, y });
    }
  }
  const inTile = (v) => ctx.setTransform(view.dpr * k, 0, 0, view.dpr * k,
    view.dpr * (v.x * side - ox), view.dpr * (v.y * side - oy));

  for (const v of vis) {
    inTile(v);
    if (v.t.water) { ctx.fillStyle = color.water; ctx.fill(v.t.water); }
    if (v.t.sand) { ctx.fillStyle = color.sand; ctx.fill(v.t.sand); }
    if (v.t.green) { ctx.fillStyle = color.green; ctx.fill(v.t.green); }
  }
  // Casings first across every tile, then fills, so junctions join cleanly.
  const w = widths(view.z);
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';
  const step = (field, widthPx, labelStyle) => {
    if (widthPx <= 0) return;
    ctx.strokeStyle = labelStyle;
    ctx.lineWidth = widthPx / k;
    for (const v of vis) if (v.t[field]) { inTile(v); ctx.stroke(v.t[field]); }
  };
  if (!dark()) {
    step('r2', w.place + 1.2, color.roadCasing);
    step('r1', w.major + 1.4, color.roadCasing);
    step('r0', w.motorway + 1.6, color.motorwayCasing);
  }
  step('r2', w.place, color.road);
  step('r1', w.major, color.road);
  step('r0', w.motorway, color.motorway);
  ctx.setTransform(view.dpr, 0, 0, view.dpr, 0, 0);
  return { vis, side, ox, oy, color };
}

/* Names on a separate pass, so a label is never cut at a tile edge, and a
   label that would overlap one already drawn is simply skipped. */
function drawNames(ctx, b) {
  loadPlaceNames();
  const boxes = [];
  const free = (x, y, w, h) => {
    for (const c of boxes) if (x < c[0] + c[2] && x + w > c[0] && y < c[1] + c[3] && y + h > c[1]) return false;
    boxes.push([x, y, w, h]);
    return true;
  };
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.lineJoin = 'round';

  // Streets first at high zoom: at a bus stop, the avenue matters more than
  // the neighbourhood you are already standing in.
  if (view.z >= 15) {
    ctx.font = `500 ${view.z >= 17 ? 12.5 : 11.5}px Roboto, system-ui, sans-serif`;
    const k = b.side / 1024;
    const seen = new Map();
    for (const v of b.vis) {
      for (const [lx, ly, ang, name, cls] of v.t.names || []) {
        if (cls > 1 && view.z < 16) continue;
        const x = v.x * b.side - b.ox + lx * k;
        const y = v.y * b.side - b.oy + ly * k;
        if (x < 0 || y < 0 || x > view.W || y > view.H) continue;
        const previous = seen.get(name);
        if (previous && Math.hypot(previous[0] - x, previous[1] - y) < 320) continue;
        const wtxt = ctx.measureText(name).width;
        const r = Math.abs(ang) * Math.PI / 180;
        const bw = Math.abs(Math.cos(r)) * wtxt + Math.abs(Math.sin(r)) * 14;
        const bh = Math.abs(Math.sin(r)) * wtxt + Math.abs(Math.cos(r)) * 14;
        if (!free(x - bw / 2, y - bh / 2, bw, bh)) continue;
        seen.set(name, [x, y]);
        ctx.save();
        ctx.translate(x, y);
        ctx.rotate(ang * Math.PI / 180);
        ctx.strokeStyle = b.color.halo; ctx.lineWidth = 3.5; ctx.strokeText(name, 0, 0);
        ctx.fillStyle = b.color.street; ctx.fillText(name, 0, 0);
        ctx.restore();
      }
    }
  }
  if (placeNames && placeNames.length) {
    const maxRank = view.z < 13 ? 1 : 2;
    const size = view.z < 13 ? 13 : 12.5;
    ctx.font = `500 ${size}px Roboto, system-ui, sans-serif`;
    for (const [lon, lat, name, rank] of placeNames) {
      if (rank > maxRank) continue;
      if (view.z >= 16 && rank < 2) continue;
      const [x, y] = toScreen(lat, lon);
      if (x < 0 || y < 0 || x > view.W || y > view.H) continue;
      const wtxt = ctx.measureText(name).width;
      if (!free(x - wtxt / 2 - 4, y - 9, wtxt + 8, 18)) continue;
      ctx.strokeStyle = b.color.halo; ctx.lineWidth = 3.5; ctx.strokeText(name, x, y);
      ctx.fillStyle = b.color.placeHit; ctx.fillText(name, x, y);
    }
  }
}

let requested = false;
function request() {
  if (requested) return;
  requested = true;
  requestAnimationFrame(() => { requested = false; drawMap(); renderCallout(); });
}
const dark = () => document.documentElement.dataset.theme === 'dark';

function glyph(ctx, x, y, size, color) {
  ctx.save();
  ctx.translate(x - size / 2, y - size / 2);
  ctx.scale(size / 24, size / 24);
  ctx.fillStyle = color;
  ctx.fill(BUS);
  ctx.restore();
}
function roundedRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function drawMap() {
  const cv = cvs();
  if (!cv || !view.W) return;
  const ctx = cv.getContext('2d');
  const night = dark();
  ctx.setTransform(view.dpr, 0, 0, view.dpr, 0, 0);
  targets = [];

  const base = drawBase(ctx);
  drawNames(ctx, base);

  const c = scene();
  const offScreen = (x, y, mg = 30) => x < -mg || y < -mg || x > view.W + mg || y > view.H + mg;

  // Routes: only the direction you ride, thin, in the line's colour.
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';
  for (const r of c.routes) {
    const w = r.strong ? 6 : 5;
    const screenPts = r.pts.map(([la, lo]) => toScreen(la, lo));
    for (const [lw, color] of [[w + 3, night ? 'rgba(26,27,30,.9)' : 'rgba(255,255,255,.9)'], [w, r.color]]) {
      ctx.lineWidth = lw;
      ctx.strokeStyle = color;
      ctx.beginPath();
      screenPts.forEach(([x, y], i) => { i ? ctx.lineTo(x, y) : ctx.moveTo(x, y); });
      ctx.stroke();
    }
    // Arrows along the line, every 90 px, pointing the way it runs: which way a
    // bus goes is the incumbent's most complained-about blind spot.
    if (view.z >= 13) routeArrows(ctx, screenPts, w);
  }

  // Small stops: tappable dots, never signs, so they do not cover the city.
  for (const s of c.dots) {
    const [x, y] = toScreen(s.lat, s.lon);
    if (offScreen(x, y)) continue;
    const r = view.z >= 17 ? 4.5 : 3.5;
    ctx.beginPath();
    ctx.arc(x, y, r, 0, 7);
    ctx.fillStyle = night ? '#202124' : '#ffffff';
    ctx.fill();
    ctx.lineWidth = 2;
    ctx.strokeStyle = s.color || (night ? '#8c9eff' : '#303f9f');
    ctx.stroke();
    targets.push({ kind: 'stop', x, y, r: 14, p: s.p || s });
  }

  // Your stops: one sign per line you follow, the one you would walk to.
  const drawSign = (x, y, t, color) => {
    roundedRect(ctx, x - t / 2, y - t / 2, t, t, t * 0.26);
    ctx.fillStyle = color;
    ctx.fill();
    ctx.lineWidth = 2;
    ctx.strokeStyle = '#ffffff';
    ctx.stroke();
    glyph(ctx, x, y, t * 0.66, '#ffffff');
  };
  const labels = [];
  // Other stops, small signs, only when zoomed in.
  for (const s of c.others) {
    const [x, y] = toScreen(s.lat, s.lon);
    if (offScreen(x, y)) continue;
    drawSign(x, y, view.z >= 17 ? 17 : 14, night ? '#5c6bc0' : '#303f9f');
    targets.push({ kind: 'stop', x, y, r: 14, p: s });
  }
  // Your stop for each line: the sign, and a label saying what it is.
  for (const s of c.yours) {
    const [x, y] = toScreen(s.lat, s.lon);
    if (offScreen(x, y, 120)) continue;
    drawSign(x, y, 24, '#303f9f');
    if (view.z >= 13) labels.push([x, y, `Parada da ${s.lines.join(', ')}`]);
    targets.push({ kind: 'stop', x, y, r: 22, p: state.stops.get(s.id) || s });
  }
  if (c.highlight) {
    const [x, y] = toScreen(c.highlight.lat, c.highlight.lon);
    ctx.beginPath(); ctx.arc(x, y, 22, 0, 7);
    ctx.fillStyle = 'rgba(48,63,159,.16)'; ctx.fill();
    drawSign(x, y, 28, '#303f9f');
  }

  // Buses: number badge plus heading arrow. Their line numbers, when shown,
  // keep clear of the stop labels, which are drawn on top.
  const taken = labels.map(([x, y, t]) => labelBox(ctx, x, y, t));
  for (const o of c.buses) {
    const hit = drawBus(ctx, o, taken);
    if (hit) targets.push({ kind: 'bus', ...hit, v: o.v, color: o.color });
  }
  // Stop labels last, so an arriving bus never hides what the stop is.
  for (const [x, y, t] of labels) drawLabel(ctx, x, y, t, night);

  // You: a blue dot when it comes from GPS, a blue pin when you chose it.
  if (state.hasPlace && !state.pickingPlace) {
    const [x, y] = toScreen(state.here[0], state.here[1]);
    if (state.place.mode === 'manual') {
      teardrop(ctx, x, y, HERE_COLOR, null, 1.25);
    } else {
      ctx.beginPath(); ctx.arc(x, y, 16, 0, 7);
      ctx.fillStyle = 'rgba(26,115,232,.16)'; ctx.fill();
      ctx.beginPath(); ctx.arc(x, y, 8, 0, 7);
      ctx.fillStyle = HERE_COLOR; ctx.fill();
      ctx.lineWidth = 3; ctx.strokeStyle = '#ffffff'; ctx.stroke();
    }
  }
  // Choosing a place: a fixed pin in the middle, the map moves under it.
  if (state.pickingPlace) teardrop(ctx, view.W / 2, view.H / 2, HERE_COLOR, null, 1.35);
}

/* A small white tag beside a map symbol, flipped to the left near the edge. */
/* Where a stop label goes: to the right of the sign, or to the left near the
   edge of the screen. */
function labelBox(ctx, x, y, txt) {
  ctx.font = '600 12px system-ui, -apple-system, Roboto, sans-serif';
  const w = ctx.measureText(txt).width + 14, h = 22;
  let lx = x + 17;
  if (lx + w > view.W - 8) lx = x - 17 - w;
  return { x0: lx, y0: y - h / 2, x1: lx + w, y1: y + h / 2 };
}

function drawLabel(ctx, x, y, txt, night) {
  const b = labelBox(ctx, x, y, txt);
  const lx = b.x0, h = b.y1 - b.y0, w = b.x1 - b.x0;
  roundedRect(ctx, lx, y - h / 2, w, h, 11);
  ctx.fillStyle = night ? '#303134' : '#ffffff';
  ctx.shadowColor = 'rgba(60,64,67,.35)';
  ctx.shadowBlur = 4;
  ctx.shadowOffsetY = 1;
  ctx.fill();
  ctx.shadowColor = 'transparent';
  ctx.shadowBlur = 0;
  ctx.shadowOffsetY = 0;
  ctx.fillStyle = night ? '#e8eaed' : '#202124';
  ctx.textAlign = 'left';
  ctx.textBaseline = 'middle';
  ctx.fillText(txt, lx + 7, y + 0.5);
}

function routeArrows(ctx, screenPts, w) {
  const step = 90;
  let acc = step / 2;
  ctx.strokeStyle = '#ffffff';
  ctx.lineWidth = Math.max(1.8, w * 0.38);
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  const s = w * 0.62;
  for (let i = 1; i < screenPts.length; i++) {
    const [ax, ay] = screenPts[i - 1], [bx, by] = screenPts[i];
    const tabs = Math.hypot(bx - ax, by - ay);
    if (tabs < 0.01) continue;
    while (acc <= tabs) {
      const t = acc / tabs;
      const x = ax + (bx - ax) * t, y = ay + (by - ay) * t;
      if (x > -20 && y > -20 && x < view.W + 20 && y < view.H + 20) {
        const ang = Math.atan2(by - ay, bx - ax);
        ctx.save();
        ctx.translate(x, y);
        ctx.rotate(ang);
        ctx.beginPath();
        ctx.moveTo(-s * 0.55, -s);
        ctx.lineTo(s * 0.55, 0);
        ctx.lineTo(-s * 0.55, s);
        ctx.stroke();
        ctx.restore();
      }
      acc += step;
    }
    acc -= tabs;
  }
}

/* The bus marker from onibus-rj (rio-map's teardrop): the body sits on the
 * bus, the point shows where it is going, a white dot in the middle. Without a
 * heading it becomes an ordinary map pin. It grows gently with the zoom. */
function teardrop(ctx, x, y, color, heading, scale) {
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(scale, scale);
  ctx.beginPath();
  if (heading != null) {
    ctx.rotate(heading * Math.PI / 180);
    ctx.moveTo(0, 10);
    ctx.bezierCurveTo(-6, 10, -11, 5.5, -11, 0);
    ctx.bezierCurveTo(-11, -6, 0, -22, 0, -22);
    ctx.bezierCurveTo(0, -22, 11, -6, 11, 0);
    ctx.bezierCurveTo(11, 5.5, 6, 10, 0, 10);
  } else {
    ctx.moveTo(0, -32);
    ctx.bezierCurveTo(-6, -32, -11, -27.5, -11, -22);
    ctx.bezierCurveTo(-11, -16, 0, 0, 0, 0);
    ctx.bezierCurveTo(0, 0, 11, -16, 11, -22);
    ctx.bezierCurveTo(11, -27.5, 6, -32, 0, -32);
  }
  ctx.closePath();
  ctx.fillStyle = color;
  ctx.fill();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = '#fff';
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(0, heading != null ? 0 : -22, 3.5, 0, Math.PI * 2);
  ctx.fillStyle = '#fff';
  ctx.fill();
  ctx.restore();
}

function drawBus(ctx, o, taken = []) {
  const v = o.v;
  const [x, y] = toScreen(v.lat, v.lon);
  if (x < -40 || y < -40 || x > view.W + 40 || y > view.H + 40) return null;
  const scale = Math.max(0.85, Math.min(1.25, 0.85 + (view.z - 13) * 0.1));
  ctx.save();
  if (v.ph === 'pending') ctx.globalAlpha = 0.45;
  teardrop(ctx, x, y, o.stale ? '#94a3b8' : o.color, v.brg, scale);
  ctx.restore();
  // The line number beside the bus, only when another line on the map has
  // the same colour: to the right, else wherever it hides no stop label and no
  // other number.
  if (o.numberTag) {
    ctx.font = TAG_FONT;
    const w = Math.ceil(ctx.measureText(o.numberTag.txt).width) + 10, h = 17;
    const options = [
      [x + 13 * scale, y - 14 * scale], [x - 13 * scale - w, y - 14 * scale],
      [x + 13 * scale, y + 8 * scale], [x - 13 * scale - w, y + 8 * scale],
    ].map(([lx, cy]) => ({ x0: lx, y0: cy - h / 2, x1: lx + w, y1: cy + h / 2 }));
    const free = options.find(b => !taken.some(q => b.x0 < q.x1 && b.x1 > q.x0 && b.y0 < q.y1 && b.y1 > q.y0)) || options[0];
    taken.push(free);
    drawNumberTag(ctx, free, o.numberTag);
  }
  // Without a heading the pin's body is above the point, so the target moves up.
  return v.brg != null ? { x, y, r: 18 * scale } : { x, y: y - 22 * scale, r: 18 * scale };
}

const TAG_FONT = '700 11px system-ui, -apple-system, Roboto, sans-serif';
function drawNumberTag(ctx, b, r) {
  ctx.save();
  ctx.font = TAG_FONT;
  roundedRect(ctx, b.x0, b.y0, b.x1 - b.x0, b.y1 - b.y0, 5);
  ctx.fillStyle = r.bg;
  ctx.fill();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = '#ffffff';
  ctx.stroke();
  ctx.fillStyle = r.text;
  ctx.textAlign = 'left';
  ctx.textBaseline = 'middle';
  ctx.fillText(r.txt, b.x0 + 5, (b.y0 + b.y1) / 2 + 0.5);
  ctx.restore();
}

/* Fit points into the part of the map that the search bar, pills or sheet
   leave visible. */
function fitPoints(pts) {
  if (!pts.length) return;
  const sheet = document.getElementById('sheet');
  const topFree = 72;
  const bottom = sheet && !sheet.hidden ? sheet.getBoundingClientRect().top : view.H - 40;
  const alt = Math.max(160, bottom - topFree - 30);
  const wd = view.W - 110;
  for (let z = 17; z >= 11; z--) {
    const xy = pts.map(([la, lo]) => proj(la, lo, z));
    const xs = xy.map(p => p[0]), ys = xy.map(p => p[1]);
    const w = Math.max(...xs) - Math.min(...xs), h = Math.max(...ys) - Math.min(...ys);
    if (w <= wd && h <= alt) {
      const cx = (Math.max(...xs) + Math.min(...xs)) / 2;
      const cy = (Math.max(...ys) + Math.min(...ys)) / 2;
      const shiftY = (topFree + bottom) / 2 - view.H / 2;
      view.z = z;
      view.center = unproject(cx, cy - shiftY, z);
      return;
    }
  }
  view.z = 11;
}

function resize() {
  const cv = cvs();
  view.dpr = Math.min(devicePixelRatio || 1, 2);
  view.W = cv.clientWidth;
  view.H = cv.clientHeight;
  cv.width = view.W * view.dpr;
  cv.height = view.H * view.dpr;
  drawMap();
}

function wireMap() {
  const cv = cvs();
  addEventListener('resize', () => { resize(); renderCallout(); });
  // Dragging pans the map and a tap picks what is under it. Your place only
  // changes through "Escolher no mapa", where the map moves under a fixed pin.
  let arr = null;
  cv.addEventListener('pointerdown', e => {
    arr = { x: e.clientX, y: e.clientY, mov: 0 };
    cv.setPointerCapture(e.pointerId);
  });
  cv.addEventListener('pointermove', e => {
    if (!arr) return;
    const dx = e.clientX - arr.x, dy = e.clientY - arr.y;
    arr.mov += Math.abs(dx) + Math.abs(dy);
    arr.x = e.clientX; arr.y = e.clientY;
    const [cx, cy] = proj(view.center[0], view.center[1], view.z);
    view.center = unproject(cx - dx, cy - dy, view.z);
    request();
  });
  cv.addEventListener('pointerup', e => {
    const a = arr;
    arr = null;
    if (!a || a.mov >= 6) return;
    const r = cv.getBoundingClientRect();
    const px = e.clientX - r.left, py = e.clientY - r.top;
    if (state.pickingPlace) {
      const [cx, cy] = proj(view.center[0], view.center[1], view.z);
      view.center = unproject(cx + px - view.W / 2, cy + py - view.H / 2, view.z);
      request();
      return;
    }
    let best = null, dm = Infinity;
    // Buses were drawn last, so they win over the stop underneath.
    for (let i = targets.length - 1; i >= 0; i--) {
      const t = targets[i];
      const d = Math.hypot(t.x - px, t.y - py);
      if (d <= t.r && (d < dm || (best && best.kind === 'stop' && t.kind === 'bus'))) {
        if (!best || t.kind === 'bus' || best.kind !== 'bus') { best = t; dm = d; }
      }
    }
    onMapTap(best);
  });
  cv.addEventListener('pointercancel', () => { arr = null; });
  cv.addEventListener('wheel', e => {
    e.preventDefault();
    view.z = Math.max(11, Math.min(18, view.z + (e.deltaY < 0 ? 1 : -1)));
    request();
  }, { passive: false });
  let pinch = null;
  cv.addEventListener('touchstart', e => {
    if (e.touches.length === 2) pinch = Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
  }, { passive: true });
  cv.addEventListener('touchmove', e => {
    if (e.touches.length !== 2 || !pinch) return;
    const d = Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
    if (Math.abs(d - pinch) > 40) { view.z = Math.max(11, Math.min(18, view.z + (d > pinch ? 1 : -1))); pinch = d; request(); }
  }, { passive: true });
  resize();
}
