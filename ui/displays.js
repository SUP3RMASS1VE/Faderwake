// The drawing surface is sized in CSS pixels. Device-pixel scaling keeps the
// one-pixel rules and small type sharp when the window moves between screens.
const mono = '10px "SFMono-Regular", Consolas, "Liberation Mono", monospace';
export const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
export const hasGain = (shape) => ['bell', 'lowShelf', 'highShelf'].includes(shape);
export const timecode = (seconds) => {
  const total = Math.max(0, Math.floor(seconds || 0));
  const clock = [Math.floor(total / 60), total % 60];
  return clock.map((value) => String(value).padStart(2, '0')).join(':');
};

export const bandColors = ['#ffc657', '#51dfcf', '#83e094', '#b8a0f5', '#f49bc8', '#7bd9f3', '#ffb66e', '#7faafb', '#e5a0e6', '#42ddbc'];
export function artworkHue(name = '') {
  let hash = 0;
  for (const character of name) hash = (hash * 31 + character.charCodeAt(0)) | 0;
  return [65, 0, 120, 240][Math.abs(hash) % 4];
}
const artwork = new Image();
artwork.src = new URL('./studio-art.jpg', import.meta.url).href;

function surface(canvas) {
  const w = canvas.clientWidth,
    h = canvas.clientHeight;
  const ratio = Math.min(3, window.devicePixelRatio || 1);
  if (!w || !h) return null;
  if (canvas.width !== Math.round(w * ratio) || canvas.height !== Math.round(h * ratio)) {
    canvas.width = Math.round(w * ratio);
    canvas.height = Math.round(h * ratio);
  }
  const ctx = canvas.getContext('2d');
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  ctx.clearRect(0, 0, w, h);
  ctx.font = mono;
  return { ctx, w, h };
}

export function drawArtwork(canvas, peaks = [], name = '') {
  const target = surface(canvas);
  if (!target) return;
  const { ctx, w, h } = target;
  if (artwork.complete && artwork.naturalWidth) {
    ctx.save();
    canvas.style.filter = `hue-rotate(${artworkHue(name)}deg)`;
    const side = Math.min(artwork.naturalWidth, artwork.naturalHeight);
    const scale = Math.max(w / side, h / side);
    ctx.drawImage(artwork, (w - side * scale) / 2, (h - side * scale) / 2, side * scale, side * scale);
    ctx.restore();
    return;
  }
  artwork.onload = () => drawArtwork(canvas, peaks, name);
  const backdrop = ctx.createLinearGradient(0, 0, w, h);
  backdrop.addColorStop(0, '#e7a164');
  backdrop.addColorStop(0.48, '#aa6241');
  backdrop.addColorStop(1, '#3d2e2a');
  ctx.fillStyle = backdrop;
  ctx.fillRect(0, 0, w, h);
  // The thumbnail's ridge follows the imported file's actual peak envelope.
  for (let layer = 0; layer < 18; layer++) {
    ctx.beginPath();
    for (let x = -1; x <= w + 1; x++) {
      const offset = clamp((x / w) * 0.85 + layer * 0.006, 0, 0.999);
      const index = Math.floor(offset * peaks.length);
      const a = ((peaks[index] || 0) + (peaks[Math.max(0, index - 1)] || 0)) / 2;
      const y = h * (0.25 + layer / 23 + 0.12 * Math.cos((x / w) * 7 + layer / 14) - a * 0.22);
      if (x === -1) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.strokeStyle = layer < 9 ? 'rgba(48,37,32,.72)' : 'rgba(246,178,118,.55)';
    ctx.lineWidth = 1;
    ctx.stroke();
  }
}

export function drawWave(canvas, track, spectrogram = null, combined = false) {
  const target = surface(canvas);
  if (!target) return;
  const { ctx, w, h } = target;
  const styles = getComputedStyle(document.documentElement);
  const top = 8,
    bottom = h - 20,
    waveBottom = combined && spectrogram ? top + (bottom - top) * 0.65 : bottom,
    cy = (top + waveBottom) / 2,
    pad = 7;
  if (spectrogram) {
    const spectrumTop = combined ? waveBottom + 4 : top;
    ctx.drawImage(spectrogram, pad, spectrumTop, w - 2 * pad, bottom - spectrumTop);
  }
  const steps = w < 410 ? 4 : 6;
  ctx.font = '9px "SFMono-Regular", Consolas, monospace';
  for (let step = 0; step <= steps; step++) {
    const x = Math.round(pad + ((w - 2 * pad) * step) / steps) + 0.5;
    ctx.strokeStyle = styles.getPropertyValue('--line').trim();
    ctx.beginPath();
    ctx.moveTo(x, 0);
    ctx.lineTo(x, bottom + 3);
    ctx.stroke();
    if (track) {
      ctx.fillStyle = styles.getPropertyValue('--muted').trim();
      ctx.textAlign = step === 0 ? 'left' : step === steps ? 'right' : 'center';
      ctx.fillText(timecode((track.duration * step) / steps), x, h - 5);
    }
  }
  if (spectrogram && !combined) return;
  ctx.strokeStyle = styles.getPropertyValue('--line').trim();
  ctx.beginPath();
  ctx.moveTo(0, Math.round(cy) + 0.5);
  ctx.lineTo(w, Math.round(cy) + 0.5);
  ctx.stroke();
  if (!track?.peaks?.length) return;
  const peaks = track.peaks;
  const columns = Math.max(1, Math.min(Math.floor(w - pad * 2), peaks.length));
  const gradient = ctx.createLinearGradient(0, top, 0, bottom);
  const light = document.documentElement.dataset.appearance === 'light';
  const teal = styles.getPropertyValue('--teal').trim() || '#26dbc7';
  gradient.addColorStop(0, light ? '#087f78' : '#7af7df');
  gradient.addColorStop(0.5, teal);
  gradient.addColorStop(1, light ? '#087f78' : '#118d84');
  ctx.fillStyle = gradient;
  const laneWidth = (w - 2 * pad) / columns;
  for (let i = 0; i < columns; i++) {
    const start = Math.floor((i * peaks.length) / columns);
    const end = Math.max(start + 1, Math.floor(((i + 1) * peaks.length) / columns));
    let peak = 0;
    for (let j = start; j < end; j++) peak = Math.max(peak, Math.abs(peaks[j]));
    const half = clamp(peak, 0, 1) * (waveBottom - top) * 0.49;
    if (half > 0)
      ctx.fillRect(pad + i * laneWidth, cy - half, Math.max(1, laneWidth * 0.85), 2 * half);
  }
}

function coefficients(band, rate) {
  if (!band.enabled || (hasGain(band.shape) && band.gain === 0)) return null;
  const w = (2 * Math.PI * Math.min(band.frequency, rate * 0.45)) / rate;
  const c = Math.cos(w),
    a = 10 ** (band.gain / 40);
  const alpha = Math.sin(w) / (2 * band.q),
    beta = 2 * Math.sqrt(a) * alpha;
  switch (band.shape) {
    case 'lowPass':
      return [(1 - c) / 2, 1 - c, (1 - c) / 2, 1 + alpha, -2 * c, 1 - alpha];
    case 'highPass':
      return [(1 + c) / 2, -(1 + c), (1 + c) / 2, 1 + alpha, -2 * c, 1 - alpha];
    case 'notch':
      return [1, -2 * c, 1, 1 + alpha, -2 * c, 1 - alpha];
    case 'lowShelf':
      return [
        a * (a + 1 - (a - 1) * c + beta),
        2 * a * (a - 1 - (a + 1) * c),
        a * (a + 1 - (a - 1) * c - beta),
        a + 1 + (a - 1) * c + beta,
        -2 * (a - 1 + (a + 1) * c),
        a + 1 + (a - 1) * c - beta,
      ];
    case 'highShelf':
      return [
        a * (a + 1 + (a - 1) * c + beta),
        -2 * a * (a - 1 + (a + 1) * c),
        a * (a + 1 + (a - 1) * c - beta),
        a + 1 - (a - 1) * c + beta,
        2 * (a - 1 - (a + 1) * c),
        a + 1 - (a - 1) * c - beta,
      ];
    default:
      return [1 + alpha * a, -2 * c, 1 - alpha * a, 1 + alpha / a, -2 * c, 1 - alpha / a];
  }
}

export function eqResponse(bands, rate) {
  const filters = bands.map((band) => coefficients(band, rate));
  return (hz, skip = -1) => {
    const omega = (2 * Math.PI * hz) / rate;
    const c = Math.cos(omega),
      s = Math.sin(omega),
      c2 = Math.cos(2 * omega),
      s2 = Math.sin(2 * omega);
    let gain = 0;
    for (let i = 0; i < filters.length; i++) {
      if (i === skip || !filters[i]) continue;
      const [b0, b1, b2, a0, a1, a2] = filters[i];
      const numerator = (b0 + b1 * c + b2 * c2) ** 2 + (b1 * s + b2 * s2) ** 2;
      const denominator = (a0 + a1 * c + a2 * c2) ** 2 + (a1 * s + a2 * s2) ** 2;
      gain += 10 * Math.log10(Math.max(1e-12, numerator / Math.max(1e-18, denominator)));
    }
    return gain;
  };
}

export function graphScale(canvas, rate) {
  const left = 36,
    top = 16,
    width = Math.max(1, canvas.clientWidth - 54);
  const height = Math.max(1, canvas.clientHeight - 41);
  const topHz = Math.min(20000, rate * 0.45),
    octaves = Math.log(topHz / 20);
  return {
    left,
    top,
    width,
    height,
    topHz,
    x: (hz) => left + (Math.log(clamp(hz, 20, topHz) / 20) / octaves) * width,
    y: (db) => top + ((18 - clamp(db, -18, 18)) / 36) * height,
    hz: (x) => 20 * Math.exp(clamp((x - left) / width, 0, 1) * octaves),
    db: (y) => 18 - clamp((y - top) / height, 0, 1) * 36,
  };
}

export function drawEq(canvas, track, selected, bypassed = false) {
  const target = surface(canvas);
  if (!target) return [];
  const { ctx, w, h } = target;
  const rate = track?.output?.sampleRate || track?.sampleRate || 48000;
  const plot = graphScale(canvas, rate);
  const bottom = plot.top + plot.height;
  const light = document.documentElement.dataset.appearance === 'light';
  const bg = ctx.createLinearGradient(0, 0, 0, h);
  bg.addColorStop(0, light ? '#f0f6f5' : '#101b21');
  bg.addColorStop(1, light ? '#e4eeec' : '#0b1419');
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, w, h);
  ctx.font = '9px "SFMono-Regular", Consolas, monospace';
  for (const db of [-18, -12, -6, 0, 6, 12, 18]) {
    const y = Math.round(plot.y(db)) + 0.5;
    ctx.strokeStyle = db === 0 ? (light ? '#92aaa6' : '#3b5358') : (light ? '#d0dfdc' : '#233239');
    ctx.beginPath();
    ctx.moveTo(plot.left, y);
    ctx.lineTo(w - 14, y);
    ctx.stroke();
    ctx.fillStyle = light ? '#4e6965' : '#99afb7';
    ctx.textAlign = 'right';
    ctx.fillText(db > 0 ? '+' + db : String(db).replace('-', '−'), plot.left - 8, y + 3);
  }
  const labels = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000];
  for (let decade = 10; decade < plot.topHz; decade *= 10) {
    for (let multiple = 1; multiple < 10; multiple++) {
      const hz = decade * multiple;
      if (hz < 20 || hz > plot.topHz) continue;
      const x = Math.round(plot.x(hz)) + 0.5;
      ctx.strokeStyle = labels.includes(hz) ? (light ? '#c6d8d3' : '#27383e') : (light ? '#dce7e4' : '#19282e');
      ctx.beginPath();
      ctx.moveTo(x, plot.top);
      ctx.lineTo(x, bottom);
      ctx.stroke();
    }
  }
  ctx.fillStyle = light ? '#4e6965' : '#99afb7';
  ctx.textAlign = 'center';
  for (const hz of labels) {
    if (hz > plot.topHz * 1.015) continue;
    ctx.fillText(hz >= 1000 ? hz / 1000 + 'k' : String(hz), plot.x(hz), h - 8);
  }
  if (!track?.bands.length) return [];
  const getResponse = eqResponse(track.bands, rate);
  const nodes = track.bands.map((band, index) => {
    const hz = clamp(band.frequency, 20, plot.topHz);
    return { index, x: plot.x(hz), y: plot.y(getResponse(hz)), hz, enabled: band.enabled };
  });
  const sampleCount = Math.max(100, Math.ceil(plot.width / 2));
  const frequencies = Array.from({ length: sampleCount + 1 }, (_, i) =>
    plot.hz(plot.left + (plot.width * i) / sampleCount),
  );
  frequencies.push(...nodes.map((node) => node.hz));
  frequencies.sort((a, b) => a - b);
  const path = new Path2D();
  frequencies.forEach((hz, i) => {
    const x = plot.x(hz),
      y = plot.y(getResponse(hz));
    if (i === 0) path.moveTo(x, y);
    else path.lineTo(x, y);
  });
  const fill = new Path2D(path);
  fill.lineTo(plot.left + plot.width, bottom);
  fill.lineTo(plot.left, bottom);
  fill.closePath();
  const tint = ctx.createLinearGradient(0, plot.top, 0, bottom);
  tint.addColorStop(0, 'rgba(38,219,199,.32)');
  tint.addColorStop(1, 'rgba(38,219,199,.02)');
  ctx.fillStyle = tint;
  ctx.fill(fill);
  ctx.lineWidth = 2;
  ctx.strokeStyle = bypassed
    ? '#929c97'
    : light ? '#087f78' : getComputedStyle(document.documentElement).getPropertyValue('--teal').trim() || '#26dbc7';
  ctx.setLineDash(bypassed ? [5, 4] : []);
  ctx.stroke(path);
  ctx.setLineDash([]);
  for (const node of [
    ...nodes.filter((n) => n.index !== selected),
    ...nodes.filter((n) => n.index === selected),
  ]) {
    const active = node.index === selected;
    const color = node.enabled && !bypassed ? bandColors[node.index] : '#71858c';
    if (active) {
      ctx.beginPath();
      ctx.arc(node.x, node.y, 15, 0, 2 * Math.PI);
      ctx.fillStyle = color + '30';
      ctx.fill();
    }
    ctx.beginPath();
    ctx.arc(node.x, node.y, active ? 10 : 8, 0, 2 * Math.PI);
    ctx.fillStyle = color;
    ctx.fill();
    ctx.lineWidth = 1;
    ctx.strokeStyle = active ? '#edf9f6' : '#0a2028';
    ctx.stroke();
    ctx.fillStyle = '#071c22';
    ctx.font = '600 9px monospace';
    ctx.textAlign = 'center';
    ctx.fillText(String(node.index + 1), node.x, node.y + 2.7);
  }
  return nodes;
}

export function spectrogramImage(data) {
  if (!data?.width || !data.height || data.levels?.length !== data.width * data.height) return null;
  const image = document.createElement('canvas');
  image.width = data.width;
  image.height = data.height;
  const ctx = image.getContext('2d'),
    rgba = ctx.createImageData(image.width, image.height);
  const palette = [
    [16, 20, 25],
    [43, 43, 75],
    [96, 62, 111],
    [173, 91, 101],
    [232, 148, 81],
    [255, 223, 151],
  ];
  for (let i = 0; i < data.levels.length; i++) {
    const f = clamp(data.levels[i] / 255, 0, 1) * (palette.length - 1),
      idx = Math.min(palette.length - 2, Math.floor(f)),
      t = f - idx;
    for (let ch = 0; ch < 3; ch++)
      rgba.data[4 * i + ch] = palette[idx][ch] * (1 - t) + palette[idx + 1][ch] * t;
    rgba.data[4 * i + 3] = 255;
  }
  ctx.putImageData(rgba, 0, 0);
  return image;
}
export function drawSpectrum(canvas, bins, rate) {
  if (!canvas) return;
  const target = surface(canvas);
  if (!target || !bins?.length) return;
  const { ctx } = target,
    plot = graphScale(canvas, rate);
  const floor = plot.top + plot.height;
  ctx.beginPath();
  ctx.moveTo(plot.left, floor);
  for (let i = 0; i < bins.length; i++) {
    const x = plot.left + (i / (bins.length - 1)) * plot.width;
    ctx.lineTo(x, floor - clamp(bins[i], 0, 1) * plot.height * 0.9);
  }
  ctx.lineTo(plot.left + plot.width, floor);
  ctx.closePath();
  const gradient = ctx.createLinearGradient(0, plot.top, 0, floor);
  gradient.addColorStop(0, 'rgba(38,219,199,.22)');
  gradient.addColorStop(1, 'rgba(38,219,199,.02)');
  ctx.fillStyle = gradient;
  ctx.fill();
  ctx.lineWidth = 1;
  ctx.strokeStyle = 'rgba(38,219,199,.38)';
  ctx.stroke();
}
