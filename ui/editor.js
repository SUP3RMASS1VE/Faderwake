const limit = (n, lo, hi) => Math.min(hi, Math.max(lo, n));
const clock = (value, digits = 3) => {
  const v = Math.max(0, value || 0),
    mins = Math.floor(v / 60);
  return (
    String(mins).padStart(2, '0') +
    ':' +
    (v % 60).toFixed(digits).padStart(digits ? 3 + digits : 2, '0')
  );
};
const editTools = [
  ['cut', 'Cut'],
  ['copy', 'Copy'],
  ['paste', 'Paste'],
  ['trim', 'Keep selection'],
  ['silence', 'Silence'],
  ['reverse', 'Reverse'],
  ['normalize', 'Normalize'],
  ['fadeIn', 'Fade in'],
  ['fadeOut', 'Fade out'],
  ['gain', 'Apply gain'],
];

export function mountEditor(container, bridge) {
  container.innerHTML = `<section class="fw-edit"><div class="ed-top"><div><span class="ed-label">Waveform editor</span><h2>Audio editor</h2></div>
    <div class="ed-buttons"><button data-cmd="undo">Undo</button><button data-cmd="redo">Redo</button><button data-cmd="reset">Reset to original</button></div></div>
    <div class="ed-player"><button data-cmd="play">Play</button><button data-cmd="stop">Stop</button>
    <span class="ed-time"></span><div class="ed-ab"><button data-cmd="original">Original</button><button data-cmd="master">Mastered</button></div></div>
    <div class="ed-display"><div class="ed-plot-toolbar"><strong data-id="channel"></strong><div class="ed-buttons"><label><input type="checkbox" data-id="snap"> Snap to zero</label>
    <button data-cmd="zoom-out" aria-label="Zoom out">−</button><output data-id="zoom">1×</output><button data-cmd="zoom-in" aria-label="Zoom in">+</button>
    <button data-cmd="fit-track">Fit</button><button data-cmd="zoom-selection">Zoom selection</button></div></div>
    <div class="ed-plot" tabindex="0" aria-label="Waveform timeline. Click the ruler to seek or drag across the channels to select audio.">
    <canvas data-id="wave"></canvas><canvas data-id="overlay"></canvas></div>
    <label class="ed-pan" hidden>Scroll timeline <input type="range" data-id="pan" min="0" step="any" max="1" value="0"></label>
    <p class="ed-help">Click the ruler to seek. Drag the waveform to select audio, or drag either edge to refine it.</p></div>
    <div class="ed-selection"><strong>Selection</strong><label>Start (s) <input type="number" data-id="start" min="0" step="0.001"></label>
    <label>End (s) <input type="number" data-id="end" min="0" step="0.001"></label><output data-id="length"></output>
    <label><input type="checkbox" data-id="loop"> Loop</label><button data-cmd="select-all">Select all</button><button data-cmd="clear">Clear</button></div>
    <div class="ed-actions"><div class="ed-top"><h3>Edit audio</h3><div class="ed-parameters"><label>Fade (s) <input type="number" data-id="fade" min=".01" max="10" step=".01" value="1"></label>
    <label>Gain (dB) <input type="number" data-id="gain" min="-24" max="24" step=".1" value="0"></label></div></div>
    <div class="ed-buttons">${editTools.map(([id, label]) => `<button data-cmd="${id}">${label}</button>`).join('')}</div></div></section>`;
  const root = container.firstElementChild,
    el = (name) => root.querySelector('[data-id="' + name + '"]');
  const button = (name) => root.querySelector('[data-cmd="' + name + '"]');
  const plot = root.querySelector('.ed-plot'),
    wave = el('wave'),
    overlay = el('overlay');
  let active = false,
    track = null,
    status = {},
    region = null,
    looping = false,
    zoom = 1,
    start = 0,
    pending = false;
  let drag = null,
    cache = '',
    peakData = [],
    loadToken = 0,
    regionToken = 0,
    waveformError = '',
    drawFrame = 0;
  let regionWrite = Promise.resolve(),
    editWrite = Promise.resolve();
  let lastSize = [0, 0],
    lastTrackObject = null,
    historyRegionKey = '';
  const duration = () => track?.duration || 0;
  const span = () => duration() / zoom;
  const maxStart = () => Math.max(0, duration() - span());
  const editable = () => !!track && !pending;
  const sourceFrame = () => 1 / (track?.sampleRate || 44100);
  const current = () => bridge.getSnapshot() || {};
  const colors = () => {
    const css = getComputedStyle(root);
    return {
      ink: css.getPropertyValue('--ink').trim() || '#e3e9e6',
      quiet: css.getPropertyValue('--muted').trim() || '#899693',
      stroke: css.getPropertyValue('--edge').trim() || '#394244',
      accent: css.getPropertyValue('--mint').trim() || '#a4d8c4',
      accent2: css.getPropertyValue('--amber').trim() || '#ffc076',
      panel: css.getPropertyValue('--panel').trim() || '#1a2022',
    };
  };
  function sizeCanvas(canvas) {
    const w = plot.clientWidth,
      h = plot.clientHeight,
      dpr = Math.min(3, window.devicePixelRatio || 1);
    const width = Math.round(w * dpr),
      height = Math.round(h * dpr);
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    const ctx = canvas.getContext('2d');
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);
    return { w, h, ctx };
  }
  const ruler = 26;
  function paintWave() {
    if (!active || !plot.clientWidth) return;
    const { ctx, w, h } = sizeCanvas(wave),
      c = colors();
    ctx.fillStyle = c.panel;
    ctx.fillRect(0, 0, w, h);
    ctx.font = '11px ui-monospace, monospace';
    ctx.fillStyle = c.quiet;
    const divisions = Math.max(2, Math.floor(w / 115));
    for (let i = 0; i <= divisions; i++) {
      const x = Math.round((w * i) / divisions) + 0.5;
      ctx.strokeStyle = c.stroke;
      ctx.beginPath();
      ctx.moveTo(x, 18);
      ctx.lineTo(x, h);
      ctx.stroke();
      ctx.textAlign = i === divisions ? 'right' : 'left';
      ctx.fillText(
        clock(start + (span() * i) / divisions, span() < 2 ? 3 : 1),
        x + (i === divisions ? -3 : 4),
        14,
      );
    }
    const lanes = track?.channels || 2,
      lane = (h - ruler) / lanes;
    for (let ch = 0; ch < lanes; ch++) {
      const top = ruler + lane * ch,
        mid = top + lane / 2;
      ctx.strokeStyle = c.stroke;
      ctx.beginPath();
      ctx.moveTo(0, top + 0.5);
      ctx.lineTo(w, top + 0.5);
      ctx.moveTo(0, mid + 0.5);
      ctx.lineTo(w, mid + 0.5);
      ctx.stroke();
      const peaks = peakData[ch] || [];
      ctx.strokeStyle = ch === 0 ? c.accent : c.accent2;
      ctx.beginPath();
      for (let i = 0; i < peaks.length; i++) {
        const x = ((i + 0.5) * w) / peaks.length;
        const [lo, hi] = peaks[i];
        ctx.moveTo(x, mid - Math.min(1, hi) * (lane * 0.44));
        ctx.lineTo(x, mid - Math.max(-1, lo) * (lane * 0.44) + 0.7);
      }
      ctx.lineWidth = Math.max(1, w / Math.max(1, peaks.length));
      ctx.stroke();
      ctx.lineWidth = 1;
      ctx.textAlign = 'left';
      ctx.fillStyle = c.quiet;
      ctx.fillText(lanes === 1 ? 'M' : ch === 0 ? 'L' : 'R', 7, top + 14);
    }
    paintOverlay();
  }
  function paintOverlay() {
    if (!active || !plot.clientWidth) return;
    const { ctx, w, h } = sizeCanvas(overlay),
      c = colors();
    if (!duration()) return;
    const x = (t) => ((t - start) / span()) * w;
    if (region) {
      const left = x(region[0]),
        right = x(region[1]);
      ctx.fillStyle = c.accent;
      ctx.globalAlpha = 0.19;
      ctx.fillRect(left, ruler, right - left, h - ruler);
      ctx.globalAlpha = 1;
      for (const edge of [left, right]) {
        ctx.fillRect(edge - 1, ruler, 2, h - ruler);
        ctx.fillRect(edge - 3, ruler, 6, 7);
      }
    }
    const pos = drag?.type === 'ruler' ? drag.time : status.position || 0,
      play = x(pos);
    ctx.fillStyle = c.accent2;
    ctx.fillRect(Math.round(play), 0, 1.5, h);
    ctx.beginPath();
    ctx.moveTo(play - 5, 0);
    ctx.lineTo(play + 5, 0);
    ctx.lineTo(play, 7);
    ctx.closePath();
    ctx.fill();
    root.querySelector('.ed-time').textContent = clock(pos);
  }
  function controls() {
    const len = duration(),
      enabled = editable(),
      has = !!region;
    for (const b of root.querySelectorAll('button,input')) b.disabled = !enabled;
    button('play').textContent = status.playing ? 'Pause' : 'Play';
    button('undo').disabled = !enabled || !(track?.canUndo ?? status.canUndo);
    button('redo').disabled = !enabled || !(track?.canRedo ?? status.canRedo);
    button('reset').disabled = !enabled || !track?.edited;
    button('gain').disabled = !enabled || !Number(el('gain').value);
    for (const cmd of ['copy', 'cut', 'trim', 'silence', 'clear', 'zoom-selection'])
      button(cmd).disabled = !enabled || !has;
    if (has && region[0] <= 0 && region[1] >= len - sourceFrame() / 2)
      button('cut').disabled = true;
    button('paste').disabled = !enabled || !(track?.canPaste ?? status.canPaste);
    button('zoom-in').disabled = !enabled || zoom >= 256;
    button('zoom-out').disabled = button('fit-track').disabled = !enabled || zoom <= 1;
    button('original').setAttribute('aria-pressed', String(!!status.original));
    button('master').setAttribute('aria-pressed', String(!status.original));
    el('loop').checked = looping && has;
    el('loop').disabled = !has || !enabled;
    el('fade').disabled = !enabled || has;
    el('start').max = el('end').max = len;
    for (const [field, val] of [
      ['start', region?.[0] ?? 0],
      ['end', region?.[1] ?? len],
    ])
      if (document.activeElement !== el(field)) el(field).value = val.toFixed(3);
    el('length').textContent = has ? (region[1] - region[0]).toFixed(3) + ' s selected' : '';
    el('zoom').textContent = Number(zoom.toFixed(1)) + '×';
    el('channel').textContent = track
      ? track.channels === 1
        ? 'Mono waveform'
        : 'Stereo waveform'
      : 'Waveform';
    el('pan').max = maxStart();
    el('pan').value = start;
    el('pan').parentElement.hidden = zoom <= 1;
  }
  function fetchWave() {
    if (!active || !track || !plot.clientWidth) return;
    const bins = Math.max(8, Math.min(4096, Math.ceil(plot.clientWidth))),
      end = Math.min(duration(), start + span());
    const key = [
      track.id,
      status.renderRevision,
      track.edited,
      status.original,
      status.rendering,
      start,
      end,
      bins,
    ].join('/');
    if (cache === key) return;
    cache = key;
    const token = ++loadToken;
    bridge
      .invoke('editor_waveform', {
        trackId: track.id,
        start,
        end,
        bins,
        original: !!status.original,
      })
      .then((data) => {
        if (token !== loadToken) return;
        peakData = data;
        waveformError = '';
        paintWave();
      })
      .catch((e) => {
        if (token === loadToken) {
          cache = '';
          if (String(e) !== waveformError) {
            waveformError = String(e);
            bridge.notify(waveformError);
          }
        }
      });
  }
  function syncView(repaint = false) {
    start = limit(start, 0, maxStart());
    controls();
    fetchWave();
    if (repaint) paintWave();
    else paintOverlay();
  }
  function update(snapshot) {
    status = snapshot.status || {};
    const next = snapshot.track || null;
    if (next?.id !== track?.id) {
      ++loadToken;
      ++regionToken;
      cache = '';
      zoom = 1;
      start = 0;
      region = next?.selection || null;
      looping = false;
      drag = null;
      peakData = [];
    }
    if (next !== lastTrackObject && !drag && !pending) {
      const key = JSON.stringify([next?.id, next?.selection || null]);
      if (key !== historyRegionKey) {
        region = next?.selection || null;
        historyRegionKey = key;
      }
      if (typeof next?.looping === 'boolean') looping = next.looping;
    }
    lastTrackObject = next;
    track = next;
    if (!active) return;
    const size = [plot.clientWidth, plot.clientHeight],
      resized = size.join('/') !== lastSize.join('/');
    lastSize = size;
    syncView(resized);
  }
  async function setRegion(value, snap = false, snapEdge = null) {
    if (!track) return;
    const id = track.id,
      token = ++regionToken;
    region = value;
    if (!region) looping = false;
    controls();
    paintOverlay();
    const targetLoop = looping;
    regionWrite = regionWrite.then(async () => {
      if (token !== regionToken || track?.id !== id) return;
      try {
        const stored = await bridge.invoke('set_region', {
          trackId: id,
          selection: value,
          snap,
          looping: targetLoop,
          snapEdge,
        });
        if (track?.id === id && token === regionToken) {
          region = stored;
          historyRegionKey = JSON.stringify([id, stored]);
          track.selection = stored;
          controls();
          paintOverlay();
        }
      } catch (error) {
        if (token === regionToken) {
          region = track?.selection || null;
          controls();
          paintOverlay();
          bridge.notify(String(error));
        }
        return false;
      }
    });
    return regionWrite;
  }
  function changeZoom(next, visibleStart) {
    const center = start + span() / 2;
    zoom = limit(next, 1, 256);
    start = visibleStart ?? center - span() / 2;
    syncView(true);
  }
  function coordinates(e) {
    const b = plot.getBoundingClientRect();
    const x = limit(e.clientX - b.left, 0, b.width);
    return { x, y: e.clientY - b.top, time: limit(start + (x / b.width) * span(), 0, duration()) };
  }
  plot.addEventListener('pointerdown', (e) => {
    if (!editable() || e.button !== 0) return;
    e.preventDefault();
    plot.focus();
    const p = coordinates(e);
    const near = (t) => (Math.abs(t - p.time) * plot.clientWidth) / span();
    const type =
      p.y < ruler
        ? 'ruler'
        : region && Math.min(near(region[0]), near(region[1])) < 9
          ? near(region[0]) <= near(region[1])
            ? 'left'
            : 'right'
          : 'new';
    drag = {
      id: e.pointerId,
      type,
      anchor: p.time,
      x: p.x,
      time: p.time,
      initial: region ? [...region] : null,
      moved: false,
    };
    plot.setPointerCapture(e.pointerId);
  });
  plot.addEventListener('pointermove', (e) => {
    if (!drag || drag.id !== e.pointerId) return;
    const p = coordinates(e);
    drag.moved ||= Math.abs(p.x - drag.x) > 2;
    drag.time = p.time;
    if (!drag.moved) return;
    if (drag.type === 'new')
      region = [Math.min(drag.anchor, p.time), Math.max(drag.anchor, p.time)];
    if (drag.type === 'left')
      region = [
        limit(drag.initial[0] + p.time - drag.anchor, 0, drag.initial[1] - sourceFrame()),
        drag.initial[1],
      ];
    if (drag.type === 'right')
      region = [
        drag.initial[0],
        limit(drag.initial[1] + p.time - drag.anchor, drag.initial[0] + sourceFrame(), duration()),
      ];
    paintOverlay();
    controls();
  });
  plot.addEventListener('pointerup', async (e) => {
    if (!drag || drag.id !== e.pointerId) return;
    const p = coordinates(e),
      d = drag;
    drag = null;
    plot.releasePointerCapture(e.pointerId);
    if (d.type === 'ruler' || !d.moved) {
      if (!d.moved && d.type === 'new') await setRegion(null);
      await bridge.seek(p.time);
    } else
      await setRegion(
        region,
        el('snap').checked,
        ['left', 'right'].includes(d.type) ? d.type : null,
      );
  });
  plot.addEventListener('pointercancel', () => {
    if (drag) {
      region = drag.initial;
      drag = null;
      controls();
      paintOverlay();
    }
  });
  for (const field of ['start', 'end'])
    el(field).onchange = () => {
      const n = el(field).valueAsNumber,
        end = region?.[1] ?? duration(),
        begin = region?.[0] ?? 0;
      if (!Number.isFinite(n)) return controls();
      setRegion(
        field === 'start'
          ? [limit(n, 0, Math.max(0, end - sourceFrame())), end]
          : [begin, limit(n, begin + sourceFrame(), duration())],
        el('snap').checked,
        field === 'start' ? 'left' : 'right',
      );
    };
  el('loop').onchange = () => {
    looping = el('loop').checked;
    setRegion(region);
  };
  el('gain').oninput = controls;
  el('pan').oninput = () => {
    start = Number(el('pan').value);
    syncView(true);
  };
  async function action(name) {
    if (!active || !track || pending || button(name)?.disabled) return;
    if (name === 'play') return bridge.togglePlaying();
    if (name === 'stop') {
      await bridge.invoke('set_playing', { playing: false });
      return bridge.seek(0);
    }
    if (['master', 'original'].includes(name)) {
      await bridge.invoke('set_audition', {
        original: name === 'original',
        matched: status.matched !== false,
      });
      return bridge.refresh();
    }
    if (name === 'select-all') return setRegion([0, duration()]);
    if (name === 'clear') return setRegion(null);
    if (name === 'zoom-in') return changeZoom(zoom * 2);
    if (name === 'zoom-out') return changeZoom(zoom / 2);
    if (name === 'fit-track') return changeZoom(1, 0);
    if (name === 'zoom-selection' && region)
      return changeZoom(duration() / (region[1] - region[0]), region[0]);
    const aliases = { 'fade-in': 'fadeIn', 'fade-out': 'fadeOut' };
    const op = aliases[name] || name;
    if (![...editTools.map((pair) => pair[0]), 'reset', 'undo', 'redo'].includes(op)) return;
    if (button(op)?.disabled) return;
    const id = track.id,
      gainDb = limit(Number(el('gain').value) || 0, -24, 24),
      fadeSeconds = limit(Number(el('fade').value) || 1, 0.01, 10);
    pending = true;
    controls();
    editWrite = (async () => {
      try {
        if ((await regionWrite) === false)
          throw Error('The selection could not be saved. Set it again before editing.');
        if (track?.id !== id) throw Error('The active track changed before the edit was applied.');
        const result = await bridge.invoke('edit_audio', {
          trackId: id,
          operation: op,
          gainDb,
          fadeSeconds,
        });
        if (result && result.id === track?.id) {
          region = result.selection ?? null;
          historyRegionKey = JSON.stringify([result.id, region]);
        }
        cache = '';
        await bridge.refresh();
        if (op === 'copy') bridge.notify('Selection copied.');
      } catch (e) {
        bridge.notify(String(e));
        return String(e);
      } finally {
        pending = false;
        update(current());
      }
    })();
    return editWrite;
  }
  plot.addEventListener('keydown', (event) => {
    if (event.metaKey || event.ctrlKey || event.altKey || event.shiftKey) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      setRegion(null);
    }
    if (event.key === 'Delete' || event.key === 'Backspace') {
      event.preventDefault();
      if (!button('cut').disabled) action('cut');
    }
    if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
      event.preventDefault();
      bridge.seek(
        (status.position || 0) + (event.key === 'ArrowRight' ? 1 : -1) * Math.min(1, span() / 10),
      );
    }
  });
  for (const b of root.querySelectorAll('[data-cmd]'))
    b.onclick = () => action(b.dataset.cmd).catch((e) => bridge.notify(String(e)));
  document.addEventListener('keydown', (e) => {
    if (
      document.querySelector('dialog[open]') ||
      !active ||
      e.defaultPrevented ||
      !(e.metaKey || e.ctrlKey) ||
      e.altKey ||
      e.target.closest('input,textarea,select,[contenteditable=true]')
    )
      return;
    const key = e.key.toLowerCase(),
      cmd = {
        c: 'copy',
        x: 'cut',
        v: 'paste',
        a: 'select-all',
        z: e.shiftKey ? 'redo' : 'undo',
        y: 'redo',
        t: 'trim',
        0: 'fit-track',
        '=': 'zoom-in',
        '+': 'zoom-in',
        '-': 'zoom-out',
      }[key];
    if (window.__TAURI__?.event && key !== 'y' && key !== '+') return;
    if (cmd) {
      e.preventDefault();
      action(cmd).catch((err) => bridge.notify(String(err)));
    }
  });
  const observe = new ResizeObserver(() => {
    if (active) {
      cancelAnimationFrame(drawFrame);
      drawFrame = requestAnimationFrame(() => syncView(true));
    }
  });
  observe.observe(plot);
  const themeChanges = new MutationObserver(() => {
    if (active) paintWave();
  });
  themeChanges.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ['style', 'class', 'data-theme', 'data-appearance'],
  });
  return {
    update,
    action,
    flush: async () => {
      const editing = pending ? editWrite : null;
      await regionWrite;
      const error = await editing;
      if (error) throw Error(error);
    },
    activate(snapshot) {
      active = true;
      update(snapshot);
      paintWave();
    },
    deactivate() {
      active = false;
      drag = null;
    },
  };
}
