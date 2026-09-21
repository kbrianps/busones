/* A small slippy map on one canvas, drawn the way Lá vem o ônibus draws its
 * map: navy stop signs, bus icons in the colour of their line, routes in the
 * same colour. Two things differ on purpose, both from the incumbent's own
 * reviews: markers grow with the zoom ("tudo aumenta de tamanho, menos os
 * ônibus"), and each bus carries a pointer showing which way it is going. */

const ONIBUS = new Path2D('M4 16c0 .88.39 1.67 1 2.22V20c0 .55.45 1 1 1h1c.55 0 1-.45 1-1v-1h8v1c0 .55.45 1 1 1h1c.55 0 1-.45 1-1v-1.78c.61-.55 1-1.34 1-2.22V6c0-3.5-3.58-4-8-4s-8 .5-8 4v10zm3.5 1c-.83 0-1.5-.67-1.5-1.5S6.67 14 7.5 14s1.5.67 1.5 1.5S8.33 17 7.5 17zm9 0c-.83 0-1.5-.67-1.5-1.5s.67-1.5 1.5-1.5 1.5.67 1.5 1.5-.67 1.5-1.5 1.5zm1.5-6H6V6h12v5z');

const mapa = { centro: [-22.9068, -43.1729], z: 15, W: 0, H: 0, dpr: 1 };
let alvos = [];

const cvs = () => document.getElementById('mapa');
const mundo = z => 256 * 2 ** z;
function proj(lat, lon, z) {
  const s = mundo(z), sin = Math.sin(lat * Math.PI / 180);
  return [(lon + 180) / 360 * s, (0.5 - Math.log((1 + sin) / (1 - sin)) / (4 * Math.PI)) * s];
}
function desproj(x, y, z) {
  const s = mundo(z), n = Math.PI - 2 * Math.PI * y / s;
  return [180 / Math.PI * Math.atan(0.5 * (Math.exp(n) - Math.exp(-n))), x / s * 360 - 180];
}
function origem() {
  const [cx, cy] = proj(mapa.centro[0], mapa.centro[1], mapa.z);
  return [cx - mapa.W / 2, cy - mapa.H / 2];
}
function naTela(lat, lon) {
  const [ox, oy] = origem(), [x, y] = proj(lat, lon, mapa.z);
  return [x - ox, y - oy];
}

/* ---------- our basemap ----------
 * Water, green areas, beaches, streets and names, drawn from small vector
 * tiles cut from OpenStreetMap by `busones base build`. Each tile's layers are
 * turned into Path2D objects once, in tile coordinates, and redrawn with a
 * transform, so panning costs a few fills per tile. */
const BASE = '/dist/base';
const CORES_BASE = {
  claro: { terra: '#f4f4f1', agua: '#c9e3f0', verde: '#dcebd3', areia: '#f3ecd8',
           via: '#ffffff', viaBorda: '#e1e2e4', expressa: '#fbe7ad', expressaBorda: '#ead08b',
           lugar: '#6f747a', rua: '#80858b', halo: 'rgba(244,244,241,.92)' },
  escuro: { terra: '#1c1d20', agua: '#152736', verde: '#1d2922', areia: '#2a2822',
            via: '#34363b', viaBorda: '#1c1d20', expressa: '#4a4231', expressaBorda: '#1c1d20',
            lugar: '#8f959b', rua: '#8a9096', halo: 'rgba(28,29,32,.92)' },
};
const tilesBase = new Map();
let lugares = null;

function zoomFonte(z) { return z < 12.5 ? 11 : z < 14.5 ? 13 : 15; }

function caminho(poligonos) {
  const p = new Path2D();
  for (const rings of poligonos) for (const r of rings) {
    p.moveTo(r[0], r[1]);
    for (let i = 2; i < r.length; i += 2) p.lineTo(r[i], r[i + 1]);
    p.closePath();
  }
  return p;
}
function linhas(ls) {
  const p = new Path2D();
  for (const l of ls) {
    p.moveTo(l[0], l[1]);
    for (let i = 2; i < l.length; i += 2) p.lineTo(l[i], l[i + 1]);
  }
  return p;
}

function tileBase(z, x, y) {
  const k = `${z}/${x}/${y}`;
  let t = tilesBase.get(k);
  if (t) return t;
  t = { pronto: false };
  tilesBase.set(k, t);
  fetch(`${BASE}/${k}.json`).then(r => r.ok ? r.json() : null).then(d => {
    if (d) {
      t.agua = d.w ? caminho(d.w) : null;
      t.verde = d.g ? caminho(d.g) : null;
      t.areia = d.s ? caminho(d.s) : null;
      t.r0 = d.r0 ? linhas(d.r0) : null;
      t.r1 = d.r1 ? linhas(d.r1) : null;
      t.r2 = d.r2 ? linhas(d.r2) : null;
      t.nomes = d.l || [];
    }
    t.pronto = true;
    pede();
  }).catch(() => { t.pronto = true; });
  if (tilesBase.size > 500) tilesBase.delete(tilesBase.keys().next().value);
  return t;
}

function carregaLugares() {
  if (lugares) return;
  lugares = [];
  fetch(`${BASE}/places.json`).then(r => r.json()).then(d => { lugares = d; pede(); }).catch(() => {});
}

/* Road widths in screen pixels, growing with the zoom like any street map. */
function larguras(z) {
  const f = Math.max(0, z - 13);
  return { expressa: 2.2 + f * 1.6, principal: 1.6 + f * 1.4, local: z >= 15 ? 0.8 + (z - 15) * 1.3 : 0 };
}

function desenhaBase(ctx) {
  const cor = CORES_BASE[escuro() ? 'escuro' : 'claro'];
  ctx.fillStyle = cor.terra;
  ctx.fillRect(0, 0, mapa.W, mapa.H);
  const sz = zoomFonte(mapa.z);
  const n = 2 ** sz;
  const lado = 256 * 2 ** (mapa.z - sz);
  const k = lado / 1024;
  const [ox, oy] = origem();
  const vis = [];
  for (let x = Math.floor(ox / lado); x <= Math.floor((ox + mapa.W) / lado); x++) {
    for (let y = Math.floor(oy / lado); y <= Math.floor((oy + mapa.H) / lado); y++) {
      if (y < 0 || y >= n || x < 0 || x >= n) continue;
      const t = tileBase(sz, x, y);
      if (t.pronto) vis.push({ t, x, y });
    }
  }
  const noTile = (v) => ctx.setTransform(mapa.dpr * k, 0, 0, mapa.dpr * k,
    mapa.dpr * (v.x * lado - ox), mapa.dpr * (v.y * lado - oy));

  for (const v of vis) {
    noTile(v);
    if (v.t.agua) { ctx.fillStyle = cor.agua; ctx.fill(v.t.agua); }
    if (v.t.areia) { ctx.fillStyle = cor.areia; ctx.fill(v.t.areia); }
    if (v.t.verde) { ctx.fillStyle = cor.verde; ctx.fill(v.t.verde); }
  }
  // Casings first across every tile, then fills, so junctions join cleanly.
  const w = larguras(mapa.z);
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';
  const passo = (campo, largura, estilo) => {
    if (largura <= 0) return;
    ctx.strokeStyle = estilo;
    ctx.lineWidth = largura / k;
    for (const v of vis) if (v.t[campo]) { noTile(v); ctx.stroke(v.t[campo]); }
  };
  if (!escuro()) {
    passo('r2', w.local + 1.2, cor.viaBorda);
    passo('r1', w.principal + 1.4, cor.viaBorda);
    passo('r0', w.expressa + 1.6, cor.expressaBorda);
  }
  passo('r2', w.local, cor.via);
  passo('r1', w.principal, cor.via);
  passo('r0', w.expressa, cor.expressa);
  ctx.setTransform(mapa.dpr, 0, 0, mapa.dpr, 0, 0);
  return { vis, lado, ox, oy, cor };
}

/* Names on a separate pass, so a label is never cut at a tile edge, and a
   label that would overlap one already drawn is simply skipped. */
function desenhaNomes(ctx, b) {
  carregaLugares();
  const caixas = [];
  const livre = (x, y, w, h) => {
    for (const c of caixas) if (x < c[0] + c[2] && x + w > c[0] && y < c[1] + c[3] && y + h > c[1]) return false;
    caixas.push([x, y, w, h]);
    return true;
  };
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.lineJoin = 'round';

  // Streets first at high zoom: at a bus stop, the avenue matters more than
  // the neighbourhood you are already standing in.
  if (mapa.z >= 15) {
    ctx.font = `500 ${mapa.z >= 17 ? 12.5 : 11.5}px Roboto, system-ui, sans-serif`;
    const k = b.lado / 1024;
    const vistos = new Map();
    for (const v of b.vis) {
      for (const [lx, ly, ang, nome, cls] of v.t.nomes || []) {
        if (cls > 1 && mapa.z < 16) continue;
        const x = v.x * b.lado - b.ox + lx * k;
        const y = v.y * b.lado - b.oy + ly * k;
        if (x < 0 || y < 0 || x > mapa.W || y > mapa.H) continue;
        const ant = vistos.get(nome);
        if (ant && Math.hypot(ant[0] - x, ant[1] - y) < 320) continue;
        const wtxt = ctx.measureText(nome).width;
        const r = Math.abs(ang) * Math.PI / 180;
        const bw = Math.abs(Math.cos(r)) * wtxt + Math.abs(Math.sin(r)) * 14;
        const bh = Math.abs(Math.sin(r)) * wtxt + Math.abs(Math.cos(r)) * 14;
        if (!livre(x - bw / 2, y - bh / 2, bw, bh)) continue;
        vistos.set(nome, [x, y]);
        ctx.save();
        ctx.translate(x, y);
        ctx.rotate(ang * Math.PI / 180);
        ctx.strokeStyle = b.cor.halo; ctx.lineWidth = 3.5; ctx.strokeText(nome, 0, 0);
        ctx.fillStyle = b.cor.rua; ctx.fillText(nome, 0, 0);
        ctx.restore();
      }
    }
  }
  if (lugares && lugares.length) {
    const maxRank = mapa.z < 13 ? 1 : 2;
    const tam = mapa.z < 13 ? 13 : 12.5;
    ctx.font = `500 ${tam}px Roboto, system-ui, sans-serif`;
    for (const [lon, lat, nome, rank] of lugares) {
      if (rank > maxRank) continue;
      if (mapa.z >= 16 && rank < 2) continue;
      const [x, y] = naTela(lat, lon);
      if (x < 0 || y < 0 || x > mapa.W || y > mapa.H) continue;
      const wtxt = ctx.measureText(nome).width;
      if (!livre(x - wtxt / 2 - 4, y - 9, wtxt + 8, 18)) continue;
      ctx.strokeStyle = b.cor.halo; ctx.lineWidth = 3.5; ctx.strokeText(nome, x, y);
      ctx.fillStyle = b.cor.lugar; ctx.fillText(nome, x, y);
    }
  }
}

let pedido = false;
function pede() {
  if (pedido) return;
  pedido = true;
  requestAnimationFrame(() => { pedido = false; desenhaMapa(); desenhaBalao(); });
}
const escuro = () => document.documentElement.dataset.tema === 'escuro';

function glifo(ctx, x, y, tam, cor) {
  ctx.save();
  ctx.translate(x - tam / 2, y - tam / 2);
  ctx.scale(tam / 24, tam / 24);
  ctx.fillStyle = cor;
  ctx.fill(ONIBUS);
  ctx.restore();
}
function retangulo(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function desenhaMapa() {
  const cv = cvs();
  if (!cv || !mapa.W) return;
  const ctx = cv.getContext('2d');
  const noite = escuro();
  ctx.setTransform(mapa.dpr, 0, 0, mapa.dpr, 0, 0);
  alvos = [];

  const base = desenhaBase(ctx);
  desenhaNomes(ctx, base);

  const c = cena();
  const fora = (x, y, mg = 30) => x < -mg || y < -mg || x > mapa.W + mg || y > mapa.H + mg;

  // Routes: only the direction you ride, thin, in the line's colour.
  ctx.lineJoin = 'round';
  ctx.lineCap = 'round';
  for (const r of c.rotas) {
    const w = r.forte ? 6 : 5;
    const tela = r.pts.map(([la, lo]) => naTela(la, lo));
    for (const [lw, cor] of [[w + 3, noite ? 'rgba(26,27,30,.9)' : 'rgba(255,255,255,.9)'], [w, r.cor]]) {
      ctx.lineWidth = lw;
      ctx.strokeStyle = cor;
      ctx.beginPath();
      tela.forEach(([x, y], i) => { i ? ctx.lineTo(x, y) : ctx.moveTo(x, y); });
      ctx.stroke();
    }
    // Arrows along the line, every 90 px, pointing the way it runs: which way a
    // bus goes is the incumbent's most complained-about blind spot.
    if (mapa.z >= 13) setasNaRota(ctx, tela, w);
  }

  // Small stops: tappable dots, never signs, so they do not cover the city.
  for (const s of c.pontinhos) {
    const [x, y] = naTela(s.lat, s.lon);
    if (fora(x, y)) continue;
    const r = mapa.z >= 17 ? 4.5 : 3.5;
    ctx.beginPath();
    ctx.arc(x, y, r, 0, 7);
    ctx.fillStyle = noite ? '#202124' : '#ffffff';
    ctx.fill();
    ctx.lineWidth = 2;
    ctx.strokeStyle = s.cor || (noite ? '#8c9eff' : '#303f9f');
    ctx.stroke();
    alvos.push({ tipo: 'parada', x, y, r: 14, p: s.p || s });
  }

  // Your stops: one sign per line you follow, the one you would walk to.
  const sinal = (x, y, t, cor) => {
    retangulo(ctx, x - t / 2, y - t / 2, t, t, t * 0.26);
    ctx.fillStyle = cor;
    ctx.fill();
    ctx.lineWidth = 2;
    ctx.strokeStyle = '#ffffff';
    ctx.stroke();
    glifo(ctx, x, y, t * 0.66, '#ffffff');
  };
  const etiquetas = [];
  // Other stops, small signs, only when zoomed in.
  for (const s of c.outras) {
    const [x, y] = naTela(s.lat, s.lon);
    if (fora(x, y)) continue;
    sinal(x, y, mapa.z >= 17 ? 17 : 14, noite ? '#5c6bc0' : '#303f9f');
    alvos.push({ tipo: 'parada', x, y, r: 14, p: s });
  }
  // Your stop for each line: the sign, and a label saying what it is.
  for (const s of c.suas) {
    const [x, y] = naTela(s.lat, s.lon);
    if (fora(x, y, 120)) continue;
    sinal(x, y, 24, '#303f9f');
    if (mapa.z >= 13) etiquetas.push([x, y, `Parada da ${s.linhas.join(', ')}`]);
    alvos.push({ tipo: 'parada', x, y, r: 22, p: est.paradas.get(s.id) || s });
  }
  if (c.destaque) {
    const [x, y] = naTela(c.destaque.lat, c.destaque.lon);
    ctx.beginPath(); ctx.arc(x, y, 22, 0, 7);
    ctx.fillStyle = 'rgba(48,63,159,.16)'; ctx.fill();
    sinal(x, y, 28, '#303f9f');
  }

  // Buses: number badge plus heading arrow.
  for (const o of c.onibus) {
    const hit = desenhaOnibus(ctx, o);
    if (hit) alvos.push({ tipo: 'onibus', ...hit, v: o.v, cor: o.cor });
  }
  // Stop labels last, so an arriving bus never hides what the stop is.
  for (const [x, y, t] of etiquetas) etiqueta(ctx, x, y, t, noite);

  // You: the blue dot when it comes from GPS, a red pin when you chose it.
  if (est.temLocal && !est.escolhendoLocal) {
    const [x, y] = naTela(est.eu[0], est.eu[1]);
    if (est.local.modo === 'manual') {
      gota(ctx, x, y, '#dc2626', null, 1.25);
    } else {
      ctx.beginPath(); ctx.arc(x, y, 16, 0, 7);
      ctx.fillStyle = 'rgba(26,115,232,.16)'; ctx.fill();
      ctx.beginPath(); ctx.arc(x, y, 8, 0, 7);
      ctx.fillStyle = '#1a73e8'; ctx.fill();
      ctx.lineWidth = 3; ctx.strokeStyle = '#ffffff'; ctx.stroke();
    }
  }
  // Choosing a place: a fixed pin in the middle, the map moves under it.
  if (est.escolhendoLocal) gota(ctx, mapa.W / 2, mapa.H / 2, '#dc2626', null, 1.35);
}

/* A small white tag beside a map symbol, flipped to the left near the edge. */
function etiqueta(ctx, x, y, txt, noite) {
  ctx.font = '600 12px system-ui, -apple-system, Roboto, sans-serif';
  const w = ctx.measureText(txt).width + 14, h = 22;
  let lx = x + 17;
  if (lx + w > mapa.W - 8) lx = x - 17 - w;
  retangulo(ctx, lx, y - h / 2, w, h, 11);
  ctx.fillStyle = noite ? '#303134' : '#ffffff';
  ctx.shadowColor = 'rgba(60,64,67,.35)';
  ctx.shadowBlur = 4;
  ctx.shadowOffsetY = 1;
  ctx.fill();
  ctx.shadowColor = 'transparent';
  ctx.shadowBlur = 0;
  ctx.shadowOffsetY = 0;
  ctx.fillStyle = noite ? '#e8eaed' : '#202124';
  ctx.textAlign = 'left';
  ctx.textBaseline = 'middle';
  ctx.fillText(txt, lx + 7, y + 0.5);
}

function setasNaRota(ctx, tela, w) {
  const passo = 90;
  let acc = passo / 2;
  ctx.strokeStyle = '#ffffff';
  ctx.lineWidth = Math.max(1.8, w * 0.38);
  ctx.lineCap = 'round';
  ctx.lineJoin = 'round';
  const s = w * 0.62;
  for (let i = 1; i < tela.length; i++) {
    const [ax, ay] = tela[i - 1], [bx, by] = tela[i];
    const seg = Math.hypot(bx - ax, by - ay);
    if (seg < 0.01) continue;
    while (acc <= seg) {
      const t = acc / seg;
      const x = ax + (bx - ax) * t, y = ay + (by - ay) * t;
      if (x > -20 && y > -20 && x < mapa.W + 20 && y < mapa.H + 20) {
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
      acc += passo;
    }
    acc -= seg;
  }
}

/* The bus marker from onibus-rj (rio-map's teardrop): the body sits on the
 * bus, the point shows where it is going, a white dot in the middle. Without a
 * heading it becomes an ordinary map pin. It grows gently with the zoom. */
function gota(ctx, x, y, cor, rumo, escala) {
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(escala, escala);
  ctx.beginPath();
  if (rumo != null) {
    ctx.rotate(rumo * Math.PI / 180);
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
  ctx.fillStyle = cor;
  ctx.fill();
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = '#fff';
  ctx.stroke();
  ctx.beginPath();
  ctx.arc(0, rumo != null ? 0 : -22, 3.5, 0, Math.PI * 2);
  ctx.fillStyle = '#fff';
  ctx.fill();
  ctx.restore();
}

function desenhaOnibus(ctx, o) {
  const v = o.v;
  const [x, y] = naTela(v.lat, v.lon);
  if (x < -40 || y < -40 || x > mapa.W + 40 || y > mapa.H + 40) return null;
  const escala = Math.max(0.85, Math.min(1.25, 0.85 + (mapa.z - 13) * 0.1));
  ctx.save();
  if (v.ph === 'pending') ctx.globalAlpha = 0.45;
  gota(ctx, x, y, o.velho ? '#94a3b8' : o.cor, v.brg, escala);
  ctx.restore();
  // Without a heading the pin's body is above the point, so the target moves up.
  return v.brg != null ? { x, y, r: 18 * escala } : { x, y: y - 22 * escala, r: 18 * escala };
}

/* Fit points into the part of the map that the search bar, pills or sheet
   leave visible. */
function enquadraPontos(pts) {
  if (!pts.length) return;
  const folha = document.getElementById('folha');
  const topoLivre = 72;
  const baixo = folha && !folha.hidden ? folha.getBoundingClientRect().top : mapa.H - 40;
  const alt = Math.max(160, baixo - topoLivre - 30);
  const larg = mapa.W - 110;
  for (let z = 17; z >= 11; z--) {
    const xy = pts.map(([la, lo]) => proj(la, lo, z));
    const xs = xy.map(p => p[0]), ys = xy.map(p => p[1]);
    const w = Math.max(...xs) - Math.min(...xs), h = Math.max(...ys) - Math.min(...ys);
    if (w <= larg && h <= alt) {
      const cx = (Math.max(...xs) + Math.min(...xs)) / 2;
      const cy = (Math.max(...ys) + Math.min(...ys)) / 2;
      const deslocY = (topoLivre + baixo) / 2 - mapa.H / 2;
      mapa.z = z;
      mapa.centro = desproj(cx, cy - deslocY, z);
      return;
    }
  }
  mapa.z = 11;
}

function dimensiona() {
  const cv = cvs();
  mapa.dpr = Math.min(devicePixelRatio || 1, 2);
  mapa.W = cv.clientWidth;
  mapa.H = cv.clientHeight;
  cv.width = mapa.W * mapa.dpr;
  cv.height = mapa.H * mapa.dpr;
  desenhaMapa();
}

function ligaMapa() {
  const cv = cvs();
  addEventListener('resize', () => { dimensiona(); desenhaBalao(); });
  let arr = null;
  // Your pin can be dragged, and holding a finger on the map puts you there.
  const PINO = 1.25;
  function noPino(px, py) {
    if (!est.temLocal || est.escolhendoLocal) return false;
    const [x, y] = naTela(est.eu[0], est.eu[1]);
    const cy = est.local.modo === 'manual' ? y - 22 * PINO : y;
    return Math.hypot(px - x, py - cy) < 26;
  }
  function pontoDoMapa(px, py) {
    const [ox, oy] = origem();
    return desproj(ox + px, oy + py, mapa.z);
  }
  cv.addEventListener('pointerdown', e => {
    const r = cv.getBoundingClientRect();
    const px = e.clientX - r.left, py = e.clientY - r.top;
    arr = { x: e.clientX, y: e.clientY, mov: 0, pino: noPino(px, py), segurou: false };
    if (arr.pino) {
      // Keep the pin's point where it was relative to the finger.
      const [x, y] = naTela(est.eu[0], est.eu[1]);
      arr.dx = x - px;
      arr.dy = y - py;
      est.local.modo = 'manual';
    } else if (!est.escolhendoLocal) {
      arr.timer = setTimeout(() => {
        if (!arr || arr.mov > 8) return;
        arr.segurou = true;
        if (navigator.vibrate) navigator.vibrate(15);
        const [la, lo] = pontoDoMapa(px, py);
        aoMoverPino(la, lo);
      }, 550);
    }
    cv.setPointerCapture(e.pointerId);
  });
  cv.addEventListener('pointermove', e => {
    if (!arr) return;
    const dx = e.clientX - arr.x, dy = e.clientY - arr.y;
    arr.mov += Math.abs(dx) + Math.abs(dy);
    arr.x = e.clientX; arr.y = e.clientY;
    if (arr.pino) {
      const r = cv.getBoundingClientRect();
      est.eu = pontoDoMapa(e.clientX - r.left + arr.dx, e.clientY - r.top + arr.dy);
      pede();
      return;
    }
    if (arr.mov > 8 && arr.timer) { clearTimeout(arr.timer); arr.timer = null; }
    const [cx, cy] = proj(mapa.centro[0], mapa.centro[1], mapa.z);
    mapa.centro = desproj(cx - dx, cy - dy, mapa.z);
    pede();
  });
  cv.addEventListener('pointerup', e => {
    const a = arr;
    arr = null;
    if (!a) return;
    if (a.timer) clearTimeout(a.timer);
    if (a.pino) {
      if (a.mov > 4) aoMoverPino(est.eu[0], est.eu[1]);
      return;
    }
    if (a.segurou || a.mov >= 6) return;
    const r = cv.getBoundingClientRect();
    const px = e.clientX - r.left, py = e.clientY - r.top;
    if (est.escolhendoLocal) {
      const [cx, cy] = proj(mapa.centro[0], mapa.centro[1], mapa.z);
      mapa.centro = desproj(cx + px - mapa.W / 2, cy + py - mapa.H / 2, mapa.z);
      pede();
      return;
    }
    let melhor = null, dm = Infinity;
    // Buses were drawn last, so they win over the stop underneath.
    for (let i = alvos.length - 1; i >= 0; i--) {
      const t = alvos[i];
      const d = Math.hypot(t.x - px, t.y - py);
      if (d <= t.r && (d < dm || (melhor && melhor.tipo === 'parada' && t.tipo === 'onibus'))) {
        if (!melhor || t.tipo === 'onibus' || melhor.tipo !== 'onibus') { melhor = t; dm = d; }
      }
    }
    aoTocarMapa(melhor);
  });
  cv.addEventListener('pointercancel', () => { if (arr && arr.timer) clearTimeout(arr.timer); arr = null; });
  cv.addEventListener('wheel', e => {
    e.preventDefault();
    mapa.z = Math.max(11, Math.min(18, mapa.z + (e.deltaY < 0 ? 1 : -1)));
    pede();
  }, { passive: false });
  let pinca = null;
  cv.addEventListener('touchstart', e => {
    if (e.touches.length === 2) pinca = Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
  }, { passive: true });
  cv.addEventListener('touchmove', e => {
    if (e.touches.length !== 2 || !pinca) return;
    const d = Math.hypot(e.touches[0].clientX - e.touches[1].clientX, e.touches[0].clientY - e.touches[1].clientY);
    if (Math.abs(d - pinca) > 40) { mapa.z = Math.max(11, Math.min(18, mapa.z + (d > pinca ? 1 : -1))); pinca = d; pede(); }
  }, { passive: true });
  dimensiona();
}
