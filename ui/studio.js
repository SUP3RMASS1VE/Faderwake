import { mountMastering } from './mastering.js';
import { mountWorkspace } from './workspace.js';
import { mountEditor } from './editor.js';
import { mountMetadata } from './metadata.js';
import { mountGlass } from './glass.js';
import {
  clamp,
  drawArtwork,
  drawEq,
  drawWave,
  drawSpectrum,
  spectrogramImage,
  eqResponse,
  graphScale,
  hasGain,
  timecode,
} from './displays.js';

const byId = (id) => document.getElementById(id);
const icons = {
  list: '<path d="M7 6h14M7 12h14M7 18h14M3 6h.1M3 12h.1M3 18h.1"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  settings: '<path d="M3 6h18M3 12h18M3 18h18"/><path d="M7 3v6m9 0v6m-7 0v6" stroke-width="3"/>',
  undo: '<path d="M8 5 3 10l5 5M3 10h11a6 6 0 0 1 6 6v3"/>',
  redo: '<path d="m16 5 5 5-5 5m5-5H10a6 6 0 0 0-6 6v3"/>',
  stop: '<rect x="5" y="5" width="14" height="14" rx="2" fill="currentColor" stroke="none"/>',
  folder:
    '<path d="M3 6.5h6l2 2h9v9.7a1.8 1.8 0 0 1-1.8 1.8H4.8A1.8 1.8 0 0 1 3 18.2z"/><path d="M3 6.5V5a1 1 0 0 1 1-1h5l2 2h7"/>',
  export: '<path d="M12 15V3m-4 4 4-4 4 4M4 14v5a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-5"/>',
  close: '<path d="m6 6 12 12M18 6 6 18"/>',
  rewind: '<path d="M5 5v14"/><path d="m18 5-9 7 9 7z" fill="currentColor" stroke="none"/>',
  play: '<path d="M6 3.8a.8.8 0 0 1 1.2-.7l12.4 8.1a1 1 0 0 1 0 1.6L7.2 20.9a.8.8 0 0 1-1.2-.7z" fill="currentColor" stroke="none"/>',
  pause:
    '<rect x="5" y="4" width="5" height="16" rx="1" fill="currentColor" stroke="none"/><rect x="14" y="4" width="5" height="16" rx="1" fill="currentColor" stroke="none"/>',
  meter: '<path d="M4 19V9m5 10V4m5 15v-7m5 7V6"/><path d="M3 13h2m3-5h2m3 8h2m3-5h2"/>',
  match: '<path d="M3 8h15m-3-3 3 3-3 3M21 16H6m3-3-3 3 3 3"/>',
  band: '<path d="M3 14h4c2.5 0 2-8 5-8s2.5 11 5 11h4"/><circle cx="12" cy="6" r="2" fill="currentColor"/>',
  down: '<path d="m5 9 7 7 7-7"/>',
  power: '<path d="M12 2v9m-5-7a8 8 0 1 0 10 0"/>',
};
function icon(name) {
  return '<svg viewBox="0 0 24 24" class="icon" aria-hidden="true">' + icons[name] + '</svg>';
}
for (const slot of document.querySelectorAll('[data-icon]'))
  slot.outerHTML = icon(slot.dataset.icon);

const presets = [
  'Balanced',
  'AI Cleanup',
  'Vocal Boost',
  'Bass Boost',
  'Bright & Wide',
  'Warm Glue',
  'Club Loud',
];
const shapes = {
  bell: 'Bell',
  lowShelf: 'Low shelf',
  highShelf: 'High shelf',
  highPass: 'High pass',
  lowPass: 'Low pass',
  notch: 'Notch',
};
const fields = ['frequency', 'gain', 'q'];
for (const name of presets) byId('preset').add(new Option(name, name));
const editedOption = new Option('Edited', 'edited');
editedOption.disabled = true;
editedOption.hidden = true;
byId('preset').add(editedOption);
for (const [value, label] of Object.entries(shapes)) byId('shape').add(new Option(label, value));

const invoke = window.__TAURI__?.core?.invoke;
let queue = [],
  tab = 'master',
  shell,
  mastering,
  controllers = {};
let gestureCounter = Date.now(),
  gesture = null,
  sourceGeneration = 0,
  visualKey = '',
  currentPeaks = null,
  currentSpec = null;
let refreshId = 0,
  demoId = null,
  loadedArtwork = null;
const getSnapshot = () => ({ track, status, settings: track?.settings, queue, tab });
function beginGesture() {
  gesture = ++gestureCounter;
}
function endGesture() {
  flushEdits();
  gesture = null;
}
function updateViews() {
  const snapshot = getSnapshot();
  mastering?.update(snapshot);
  shell?.render(snapshot);
  if (track) controllers[tab]?.update?.(snapshot);
}
function setTab(value) {
  const changing = tab !== value;
  if (changing) controllers[tab]?.deactivate?.();
  tab = value;
  document
    .querySelectorAll('[data-tab]')
    .forEach((b) => b.setAttribute('aria-pressed', String(b.dataset.tab === tab)));
  for (const name of ['editor', 'metadata']) byId(name + '-pane').hidden = tab !== name || !track;
  for (const selector of ['.source', '.meter-panel'])
    document.querySelector(selector).hidden = tab === 'editor';
  for (const selector of ['.master-toolbar', '#output-strip'])
    document.querySelector(selector).hidden = tab !== 'master' || !track;
  mastering?.layout();
  updateViews();
  if (changing) controllers[tab]?.activate?.(getSnapshot());
  drawSource();
  drawGraph();
}
async function refresh() {
  if (!invoke) return;
  // A completed audio edit refreshes its own view before other actions resume.
  await controllers.metadata?.flush?.();
  await flushEdits();
  const id = ++refreshId,
    revision = localRevision;
  const [next, list, fresh] = await Promise.all([
    invoke('get_track'),
    invoke('get_queue'),
    invoke('get_status'),
  ]);
  if (id !== refreshId || revision !== localRevision) return;
  queue = list;
  applyTrack(next, demoId !== null && next?.id === demoId, fresh);
}
async function updateQueue() {
  if (!invoke) return;
  queue = await invoke('get_queue');
  updateViews();
}
async function mutate(cmd, args) {
  if (!invoke || busy) return;
  busy = cmd === 'select_track' ? 'Opening track' : 'Updating session';
  syncAvailability();
  try {
    await flushForDelivery();
    const result = await invoke(cmd, args);
    if (['apply_to_all', 'remove_track'].includes(cmd)) shell.recordNative();
    await refresh();
    return result;
  } catch (error) {
    await updateQueue();
    throw error;
  } finally {
    busy = '';
    syncAvailability();
  }
}
async function setOutput(output) {
  const id = track?.id;
  if (!id) return;
  await flushEdits();
  await invoke('set_output', { trackId: id, output });
  if (track?.id !== id) return;
  track.output = output;
  derivePresetName();
  updatePresetName();
  shell.recordNative();
  await updateQueue();
  updateViews();
  drawGraph();
}
let track = null,
  selected = 0,
  edited = false,
  busy = '',
  pollInFlight = false;
let status = { position: 0, playing: false, original: false, matched: true, rendering: false };
let nodes = [],
  drag = null,
  scrubbing = false,
  lastError = '',
  timer = 0,
  pending = null,
  writeTask = null;
let localRevision = Date.now(),
  statusGeneration = 0,
  auditionPending = false,
  transportPending = false,
  seekPending = false;
let loadingPreset = false,
  failedEdit = false;

const cursor = document.createElement('div');
cursor.className = 'playhead';
cursor.hidden = true;
byId('wave').after(cursor);
for (let i = 0; i < 10; i++) {
  const button = document.createElement('button');
  button.textContent = String(i + 1).padStart(2, '0');
  button.title = 'Select band ' + (i + 1);
  button.setAttribute('aria-label', button.title);
  button.disabled = true;
  button.addEventListener('click', () => selectBand(i));
  byId('bands').append(button);
}

function notice(text) {
  byId('notice-text').textContent = text;
  byId('notice').hidden = !text;
}
byId('dismiss').onclick = () => notice('');
function report(error) {
  const message = error instanceof Error ? error.message : String(error);
  notice(message);
}
function footerState() {
  let message = busy;
  if (!message && failedEdit) message = 'Master update failed';
  if (!message && (status.rendering || pending || writeTask)) message = 'Rendering master';
  if (!message && track)
    message = status.playing ? (status.original ? 'Playing original' : 'Playing master') : 'Ready';
  byId('engine-text').textContent =
    message || (invoke ? 'Open audio files to begin' : 'Desktop audio engine unavailable');
  byId('engine-state').className = failedEdit
    ? 'error'
    : busy || status.rendering || pending || writeTask
      ? 'busy'
      : '';
}
function syncAvailability() {
  const locked = !track || !!busy;
  const band = track?.bands[selected];
  for (const id of ['desk', 'master-toolbar', 'editor-pane', 'metadata-pane']) byId(id).inert = !!busy;
  for (const id of ['open', 'demo', 'queue-add', 'start-open', 'start-demo'])
    byId(id).disabled = !!busy || !invoke;
  for (const id of ['original', 'master', 'match']) byId(id).disabled = locked || auditionPending;
  byId('play').disabled = locked || transportPending;
  byId('rewind').disabled = byId('stop').disabled = locked;
  byId('seek').disabled = locked;
  byId('export').disabled = locked || status.rendering || !!pending || !!writeTask || failedEdit;
  byId('preset').disabled = locked || loadingPreset;
  for (const id of ['shape', 'enabled']) byId(id).disabled = locked || !band || loadingPreset;
  for (const field of fields) {
    const disabled = locked || !band || loadingPreset || (field === 'gain' && !hasGain(band.shape));
    byId(field).disabled = byId(field + '-slider').disabled = disabled;
  }
  [...byId('bands').children].forEach(
    (button, i) => (button.disabled = locked || !track?.bands[i]),
  );
  footerState();
  updateViews();
}

function applyTrack(next, demo, fresh) {
  const changed = !next || next.id !== track?.id;
  if (changed) {
    drag = null;
    gesture = null;
    selected = 0;
    sourceGeneration++;
  }
  track = next;
  failedEdit = false;
  localRevision++;
  statusGeneration++;
  lastError = '';
  status = fresh ?? {
    position: 0,
    playing: false,
    original: false,
    matched: true,
    rendering: true,
  };
  if (!track) {
    setTab('master');
    syncAvailability();
    return;
  }
  if (demo) demoId = track.id;
  track.bands = track.settings.bands;
  derivePresetName();
  if (changed)
    selected = Math.max(
      0,
      track.bands.findIndex((b) => b.gain !== 0),
    );
  byId('track-name').textContent = track.name;
  byId('track-name').title = track.fileName || track.name;
  byId('source-type').textContent = demo ? 'Demo track' : 'Source track';
  const channels =
    track.channels === 1 ? 'Mono' : track.channels === 2 ? 'Stereo' : track.channels + ' channels';
  const type = track.fileName?.match(/\.([a-z0-9]+)$/i)?.[1]?.toUpperCase();
  byId('track-format').textContent = [
    type,
    channels,
    Number((track.sampleRate / 1000).toFixed(3)) + ' kHz',
    timecode(track.duration),
  ]
    .filter(Boolean)
    .join('  /  ');
  byId('seek').max = track.duration;
  byId('duration').textContent = timecode(track.duration);
  visualKey = '';
  loadedArtwork = null;
  if (changed) notice('');
  updatePresetName();
  selectBand(selected);
  setTab(tab);
  paintStatus();
  syncAvailability();
}
function drawSource() {
  if (!track) return;
  if (loadedArtwork !== track.peaks) {
    drawArtwork(byId('track-art'), track.peaks);
    loadedArtwork = track.peaks;
  }
  drawWave(byId('wave'), { ...track, peaks: currentPeaks || track.peaks }, currentSpec);
  showPosition(status.position);
}
async function loadVisual() {
  if (!track || !invoke || tab === 'editor') return;
  const original = status.original,
    kind = byId('visual').value;
  const key = [
    track.id,
    sourceGeneration,
    status.renderRevision,
    original,
    kind,
    status.rendering,
  ].join('/');
  if (key === visualKey) return;
  visualKey = key;
  currentPeaks = null;
  currentSpec = null;
  drawSource();
  if (!original && status.rendering) return;
  const id = track.id;
  try {
    if (kind === 'spectrogram') {
      const data = await invoke('spectrogram', { trackId: id, original });
      if (key !== visualKey) return;
      currentSpec = spectrogramImage(data);
    } else if (!original) {
      const peaks = await invoke('get_overview', { trackId: id, original: false });
      if (key !== visualKey) return;
      currentPeaks = peaks;
    }
    drawSource();
  } catch (e) {
    if (key === visualKey && !status.rendering) report(e);
  }
}
byId('visual').onchange = loadVisual;
function drawGraph() {
  const bypassed = status.original || track?.settings?.eqBypassed;
  nodes = drawEq(byId('graph'), track, selected, bypassed);
  byId('graph-state').textContent = bypassed
    ? 'Saved response · bypassed'
    : status.playing
      ? 'Live spectrum · relative level'
      : '';
  drawSpectrum(
    byId('spectrum-layer'),
    status.playing ? status.spectrum : [],
    track?.output?.sampleRate || track?.sampleRate || 48000,
  );
}
function selectBand(index) {
  selected = index;
  updateBandLabels();
  refreshInspector();
  drawGraph();
  syncAvailability();
}
function updateBandLabels() {
  [...byId('bands').children].forEach((button, i) => {
    button.setAttribute('aria-pressed', String(i === selected));
    const band = track?.bands[i];
    button.classList.toggle('off', !!band && !band.enabled);
    if (band) {
      const hz = Math.round(band.frequency),
        summary = (hz >= 1000 ? Number((hz / 1000).toFixed(1)) + 'k' : hz) + ' Hz';
      const detail = !band.enabled
        ? 'Off'
        : hasGain(band.shape)
          ? (band.gain > 0 ? '+' : '') + Number(band.gain.toFixed(1)) + ' dB'
          : shapes[band.shape];
      button.dataset.summary = summary;
      button.dataset.detail = detail;
      button.title = 'Band ' + (i + 1) + ': ' + summary + ', ' + shapes[band.shape] + ', ' + detail;
      button.setAttribute('aria-label', button.title);
    }
  });
}
function refreshInspector() {
  const b = track?.bands[selected];
  if (!b) return;
  byId('band-id').textContent = String(selected + 1).padStart(2, '0');
  byId('shape').value = b.shape;
  byId('enabled').checked = b.enabled;
  for (const key of fields) {
    const input = byId(key),
      slider = byId(key + '-slider');
    if (document.activeElement !== input)
      input.value =
        key === 'frequency' ? String(Math.round(b[key])) : b[key].toFixed(key === 'q' ? 2 : 1);
    slider.value =
      key === 'frequency'
        ? (Math.log(b[key] / 20) / Math.log(1000)) * 1000
        : key === 'q'
          ? (Math.log(b.q / 0.2) / Math.log(60)) * 1000
          : b[key];
    const fraction =
      (Number(slider.value) - Number(slider.min)) / (Number(slider.max) - Number(slider.min));
    slider.style.setProperty('--fraction', fraction * 100 + '%');
    slider.setAttribute(
      'aria-valuetext',
      key === 'frequency' ? Math.round(b[key]) + ' hertz' : String(b[key]),
    );
  }
}
function derivePresetName() {
  if (!track || !shell) return;
  const name = shell.matchPreset(track.settings, track.output);
  track.preset = name || 'Custom';
  edited = !name;
}
function updatePresetName() {
  editedOption.hidden = !edited;
  if (edited) editedOption.textContent = 'Custom chain';
  byId('preset').value = edited ? 'edited' : track.preset;
}
function showPosition(seconds) {
  const pct = track?.duration ? clamp(seconds / track.duration, 0, 1) : 0;
  byId('clock').textContent = timecode(seconds);
  byId('seek').value = seconds || 0;
  byId('seek').setAttribute('aria-valuetext', timecode(seconds));
  cursor.hidden = !track;
  cursor.style.left = 'calc(' + 100 * pct + '% + ' + (7 - pct * 14) + 'px)';
}
function markEdit({ instant = false, presetChange = false, output = null } = {}) {
  if (!presetChange) {
    derivePresetName();
    updatePresetName();
  }
  const id = ++localRevision;
  pending = {
    revision: id,
    trackId: track.id,
    settings: structuredClone(track.settings),
    preset: track.preset,
    gestureId: gesture ?? ++gestureCounter,
  };
  if (output) pending.output = structuredClone(output);

  failedEdit = false;
  updateBandLabels();
  refreshInspector();
  drawGraph();
  syncAvailability();
  clearTimeout(timer);
  if (instant) flushEdits();
  else timer = setTimeout(flushEdits, 150);
}
function flushEdits() {
  clearTimeout(timer);
  if (!pending) return writeTask || Promise.resolve();
  // Continuous slider movement is debounced in markEdit. Once a button click
  // or a finished gesture is submitted, later settings must not replace it.
  const { revision, ...payload } = pending;
  pending = null;
  const previous = writeTask;
  const writing = (async () => {
    if (previous) await previous;
    try {
      await invoke('update_settings', { ...payload, editId: revision });
      shell.recordNative(payload.gestureId);
      if (revision === localRevision) {
        status.rendering = true;
        statusGeneration++;
      }
    } catch (error) {
      if (revision === localRevision) {
        failedEdit = true;
        report(error);
      }
    }
  })().finally(() => {
    if (writeTask !== writing) return;
    writeTask = null;
    syncAvailability();
    poll();
    updateQueue().catch(report);
  });
  writeTask = writing;
  syncAvailability();
  return writing;
}
async function loadTrack(demo) {
  if (busy || !invoke) return;
  busy = demo ? 'Opening demo' : 'Opening audio';
  syncAvailability();
  try {
    await flushForDelivery();
    const next = await invoke(demo ? 'open_demo' : 'open_audio');
    if (next && demo) demoId = next.id;
    await refresh();
    if (next && demo) setTab('master');
  } catch (error) {
    report(error);
  } finally {
    busy = '';
    syncAvailability();
    poll();
  }
}
byId('open').onclick = () => loadTrack(false);
byId('demo').onclick = () => loadTrack(true);
function fixedMetric(value) {
  return typeof value === 'number' && Number.isFinite(value)
    ? value.toFixed(1).replace('-', '−')
    : '—';
}
function paintStatus() {
  if (!scrubbing && !seekPending) showPosition(status.position || 0);
  const play = byId('play'),
    playLabel = status.playing ? 'Pause' : 'Play';
  if (play.getAttribute('aria-label') !== playLabel) {
    play.setAttribute('aria-label', playLabel);
    play.title = playLabel + ' (Space)';
    play.innerHTML = icon(status.playing ? 'pause' : 'play');
  }
  byId('original').setAttribute('aria-pressed', String(!!status.original));
  byId('master').setAttribute('aria-pressed', String(!status.original));
  byId('match').checked = !!status.matched;
  const orig = status.original;
  byId('lufs-label').textContent = orig ? 'Original' : 'Master';
  byId('input-lufs').textContent = fixedMetric(status.originalLufs ?? track?.originalLufs);
  byId('lufs').textContent = fixedMetric(orig ? status.originalLufs : status.masterLufs);
  byId('true-peak').textContent = fixedMetric(
    orig ? status.originalTruePeak : status.masterTruePeak,
  );
  for (const [id, v] of [
    ['momentary', status.momentaryLufs],
    ['short-term', status.shortTermLufs],
    ['range', orig ? status.originalRange : status.masterRange],
    ['phase', status.playing ? status.correlation : null],
  ])
    byId(id).textContent = fixedMetric(v);
  if (status.playing && Number.isFinite(status.correlation))
    byId('phase').textContent = (status.correlation > 0 ? '+' : '') + status.correlation.toFixed(2);
  byId('true-peak').classList.toggle(
    'clipped',
    (orig ? status.originalTruePeak : status.masterTruePeak) > 0,
  );
  if (tab === 'master')
    drawSpectrum(
      byId('spectrum-layer'),
      status.playing ? status.spectrum : [],
      track?.output?.sampleRate || track?.sampleRate || 48000,
    );
  byId('graph-state').textContent =
    orig || track?.settings.eqBypassed
      ? 'Saved response · bypassed'
      : status.playing
        ? 'Live spectrum · relative level'
        : '';
  loadVisual();
  for (const [id, value] of [
    ['meter-left', status.peakLeft],
    ['meter-right', status.peakRight],
  ]) {
    const db = clamp(20 * Math.log10(Math.max(1e-8, value || 0)), -60, 0),
      meter = byId(id);
    meter.setAttribute('aria-valuenow', db.toFixed(0));
    const reading = meter.querySelector('output'),
      clipped = value >= 1;
    reading.textContent = clipped ? 'CLIP' : value > 0.001 ? db.toFixed(1) : '−∞';
    reading.classList.toggle('clipped', clipped);
    meter.querySelector('.meter-rail').style.setProperty('--level', ((db + 60) / 60) * 100 + '%');
  }
  syncAvailability();
}
async function poll() {
  if (!invoke || pollInFlight || document.hidden) return;
  const generation = statusGeneration;
  pollInFlight = true;
  try {
    const next = await invoke('get_status');
    if (next.trackId !== (track?.id || 0) || generation !== statusGeneration) return;
    status = next;
    paintStatus();
    if (next.error && next.error !== lastError) report(next.error);
    lastError = next.error || '';
  } catch (error) {
    const message = String(error);
    if (message !== lastError) report(error);
    lastError = message;
  } finally {
    pollInFlight = false;
  }
}
async function togglePlaying() {
  if (!track || busy || transportPending) return;
  transportPending = true;
  syncAvailability();
  try {
    await invoke('set_playing', { playing: !status.playing });
    statusGeneration++;
  } catch (error) {
    report(error);
  } finally {
    transportPending = false;
    syncAvailability();
    poll();
  }
}
async function audition(original, matched) {
  if (!track || busy || auditionPending) return;
  auditionPending = true;
  syncAvailability();
  try {
    await invoke('set_audition', { original, matched });
    statusGeneration++;
    status.original = original;
    status.matched = matched;
  } catch (error) {
    report(error);
  } finally {
    auditionPending = false;
    paintStatus();
    drawGraph();
    poll();
  }
}
async function seek(seconds) {
  if (!track || busy) return;
  const to = clamp(seconds, 0, track.duration);
  const id = track.id;
  seekPending = true;
  showPosition(to);
  try {
    await invoke('seek', { seconds: to });
    if (track?.id === id) {
      status.position = to;
      statusGeneration++;
    }
  } catch (error) {
    report(error);
  } finally {
    seekPending = false;
    poll();
  }
}
byId('play').onclick = togglePlaying;
byId('rewind').onclick = () => seek(0);
byId('original').onclick = () => audition(true, status.matched);
byId('master').onclick = () => audition(false, status.matched);
byId('match').onchange = () => audition(status.original, byId('match').checked);
const seekBar = byId('seek');
seekBar.oninput = () => {
  scrubbing = true;
  showPosition(Number(seekBar.value));
};
seekBar.onchange = () => {
  scrubbing = false;
  seek(Number(seekBar.value));
};
seekBar.onpointercancel = () => {
  scrubbing = false;
  showPosition(status.position);
};

async function applyPreset(preset, definition, id = track?.id) {
  if (!id || loadingPreset || busy) return;
  loadingPreset = true;
  // Only an early startup action can precede the native preset catalogue.
  if (!definition) busy = 'Loading preset';
  syncAvailability();
  try {
    const settings = definition
      ? structuredClone(definition.settings)
      : await invoke('get_preset_settings', { preset });
    const output = structuredClone(
      definition?.output || { format: 'wav', sampleRate: 44100, bitDepth: 24 },
    );
    if (track?.id !== id) {
      const token = ++gestureCounter;
      await flushForDelivery();
      await invoke('update_settings', {
        trackId: id,
        settings,
        preset,
        output,
        editId: ++localRevision,
        gestureId: token,
      });
      shell.recordNative(token);
      await updateQueue();
      return;
    }
    track.settings = settings;
    track.bands = settings.bands;
    track.preset = preset;
    edited = false;
    track.output = output;
    derivePresetName();
    updatePresetName();
    selectBand(selected);
    markEdit({ instant: true, presetChange: true, output: track.output });
    await flushEdits();
  } catch (e) {
    report(e);
    updatePresetName();
  } finally {
    if (!definition) busy = '';
    loadingPreset = false;
    syncAvailability();
  }
}
byId('preset').onchange = (e) => shell.applyPreset(e.target.value).catch(report);
byId('enable-eq').onchange = () => {
  if (track) {
    track.settings.eqBypassed = !byId('enable-eq').checked;
    markEdit({ instant: true });
  }
};
byId('reset-eq').onclick = async () => {
  if (!track) return;
  const id = track.id;
  try {
    const def = await invoke('get_preset_settings', { preset: 'Balanced' });
    if (track?.id !== id) return;
    track.bands = track.settings.bands = def.bands;
    selectBand(selected);
    markEdit({ instant: true });
  } catch (e) {
    report(e);
  }
};
for (const key of fields) {
  const input = byId(key),
    slider = byId(key + '-slider');
  input.onfocus = () => {
    input.dataset.trackId = String(track?.id);
    input.dataset.bandIndex = String(selected);
  };
  input.onchange = () => {
    if (!track || input.disabled) return;
    if (
      input.dataset.trackId &&
      (input.dataset.trackId !== String(track.id) || input.dataset.bandIndex !== String(selected))
    ) {
      input.value = track.bands[selected][key];
      return;
    }
    if (!Number.isFinite(input.valueAsNumber)) {
      input.value = track.bands[selected][key];
      return;
    }
    const val = clamp(input.valueAsNumber, Number(input.min), Number(input.max));
    input.value = val;
    if (track.bands[selected][key] === val) return;
    track.bands[selected][key] = val;
    markEdit({ instant: true });
  };
  input.onkeydown = (event) => {
    if (event.key === 'Escape' && track) {
      input.value = track.bands[selected][key];
      input.blur();
      return;
    }
    if (event.key === 'Enter') input.blur();
  };
  slider.onpointerdown = beginGesture;
  slider.onpointerup = endGesture;
  slider.onpointercancel = endGesture;
  slider.oninput = () => {
    if (!track || slider.disabled) return;
    const v = Number(slider.value);
    track.bands[selected][key] =
      key === 'frequency' ? 20 * 1000 ** (v / 1000) : key === 'q' ? 0.2 * 60 ** (v / 1000) : v;
    markEdit();
  };
  slider.onchange = flushEdits;
}
byId('shape').onchange = () => {
  if (track) {
    track.bands[selected].shape = byId('shape').value;
    markEdit({ instant: true });
  }
};
byId('enabled').onchange = () => {
  if (!track) return;
  track.bands[selected].enabled = byId('enabled').checked;
  selectBand(selected);
  markEdit({ instant: true });
};

const graph = byId('graph');
function graphPoint(event) {
  const rect = graph.getBoundingClientRect();
  return { x: event.clientX - rect.left, y: event.clientY - rect.top };
}
function hit(point) {
  let nearest,
    distance = 15;
  for (const node of nodes) {
    const d = Math.hypot(point.x - node.x, point.y - node.y);
    if (d < distance || (d === distance && node.index === selected)) {
      nearest = node;
      distance = d;
    }
  }
  return nearest;
}
graph.onpointerdown = (event) => {
  if (event.button !== 0 || !track || busy || loadingPreset || drag) return;
  const pointer = graphPoint(event),
    node = hit(pointer);
  if (!node) return;
  beginGesture();
  selectBand(node.index);
  graph.setPointerCapture(event.pointerId);
  const rate = track.output?.sampleRate || track.sampleRate;
  const response = eqResponse(track.bands, rate);
  drag = {
    id: event.pointerId,
    trackId: track.id,
    rate,
    node,
    pointer,
    response,
    original: { ...track.bands[selected] },
    changed: false,
  };
  graph.style.cursor = 'grabbing';
};
graph.onpointermove = (event) => {
  const point = graphPoint(event);
  if (!drag || event.pointerId !== drag.id) {
    graph.style.cursor = hit(point) ? 'grab' : 'default';
    return;
  }
  const dx = point.x - drag.pointer.x,
    dy = point.y - drag.pointer.y;
  if (!dx && !dy && !drag.changed) return;
  if (!track || busy || track.id !== drag.trackId) return;
  const plot = graphScale(graph, drag.rate);
  const band = track.bands[selected];
  const hz = plot.hz(drag.node.x + dx),
    start = drag.node.hz;
  band.frequency = dx === 0 || Math.abs(hz - start) < 1e-7 ? drag.original.frequency : hz;
  if (band.enabled && hasGain(band.shape)) {
    const scale = band.shape === 'bell' ? 1 : 2;
    const otherStart = drag.response(start, selected);
    const otherEnd = drag.response(Math.min(plot.topHz, band.frequency), selected);
    const vertical = (-dy / plot.height) * 36,
      initial = otherStart + drag.original.gain / scale;
    const clipped =
      initial > 18 && vertical < 0
        ? 18 - initial
        : initial < -18 && vertical > 0
          ? -18 - initial
          : 0;
    band.gain = clamp(
      drag.original.gain + (vertical + clipped + otherStart - otherEnd) * scale,
      -18,
      18,
    );
  }
  drag.changed = true;
  markEdit();
};
function endDrag(event) {
  if (!drag || event.pointerId !== drag.id) return;
  const changed = drag.changed;
  drag = null;
  graph.style.cursor = 'grab';
  if (graph.hasPointerCapture(event.pointerId)) graph.releasePointerCapture(event.pointerId);
  if (changed) flushEdits();
  gesture = null;
}
graph.onpointerup = endDrag;
graph.onpointercancel = endDrag;
graph.onlostpointercapture = endDrag;
document.addEventListener('keydown', (event) => {
  if (
    document.querySelector('dialog[open]') ||
    event.defaultPrevented ||
    event.metaKey ||
    event.ctrlKey ||
    event.altKey ||
    busy ||
    !track
  )
    return;
  if (event.target.closest('input,select,textarea,[contenteditable=true]')) return;
  if (event.code === 'Space' && event.target.closest('button:focus-visible')) return;
  if (event.code === 'Space') {
    event.preventDefault();
    if (!event.repeat) togglePlaying();
  }
  if (event.key === 'Home') {
    event.preventDefault();
    seek(0);
  }
  if (event.key === 'ArrowRight' || event.key === 'ArrowLeft') {
    event.preventDefault();
    seek(status.position + (event.key === 'ArrowRight' ? 5 : -5));
  }
});

async function stop() {
  if (!track) return;
  try {
    await invoke('set_playing', { playing: false });
    await seek(0);
  } catch (e) {
    report(e);
  }
}
byId('stop').onclick = stop;
async function flushForDelivery() {
  await controllers.editor?.flush?.();
  await controllers.metadata?.flush?.();
  await flushEdits();
}
const bridge = {
  invoke: async (cmd, args) => {
    const result = await invoke(cmd, args);
    if (cmd === 'set_tags' && track?.id === args.trackId) track.tags = structuredClone(args.tags);
    if (cmd === 'set_tags' || (cmd === 'apply_to_all' && args?.what === 'tags'))
      shell?.recordNative();
    return result;
  },
  refresh,
  notify: notice,
  getSnapshot,
  updatePreset: () => {
    derivePresetName();
    if (track) updatePresetName();
    updateViews();
  },
  seek,
  togglePlaying,
  stop,
  audition,
  mutate,
  setTab,
  load: loadTrack,
  applyPreset,
  setOutput,
  beginGesture,
  endGesture,
  flush: flushForDelivery,
  edit: (instant) => markEdit({ instant }),
  drawGraph,
  redraw: () => {
    drawGraph();
    loadedArtwork = null;
    drawSource();
  },
  tabAction: (name) => controllers[tab]?.action?.(name),
};
mastering = mountMastering(bridge);
shell = mountWorkspace(bridge);
mountGlass(bridge);
controllers.editor = mountEditor(byId('editor-pane'), bridge);
controllers.metadata = mountMetadata(byId('metadata-pane'), bridge);
shell.applyTheme();
setTab('master');
const observer = new ResizeObserver(() => {
  loadedArtwork = null;
  drawSource();
  drawGraph();
});
for (const id of ['wave', 'graph']) observer.observe(byId(id));
document.addEventListener('visibilitychange', () => {
  if (!document.hidden) poll();
});
syncAvailability();
if (invoke) {
  const events = window.__TAURI__.event;
  if (events) {
    events.listen('studio-action', (e) => shell.action(e.payload).catch(report)).catch(report);
    events.listen('queue-changed', () => refresh().catch(report)).catch(report);
    events
      .listen('studio-error', (e) => {
        report(e.payload);
        updateQueue().catch(report);
      })
      .catch(report);
  }
  const refreshMeters = async () => {
    await poll();
    const active = status.playing || status.rendering || status.exporting || pending || writeTask;
    setTimeout(refreshMeters, active && !document.hidden ? 100 : 550);
  };
  shell.loadPresets().then(refresh).catch(report).finally(refreshMeters);
} else notice('Open Faderwake in the desktop app to load and master audio.');
