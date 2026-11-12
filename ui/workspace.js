const $ = (id) => document.getElementById(id);
const palettes = {
  studio: ['Studio', '#ffc076'],
  system: ['System', 'AccentColor'],
  ocean: ['Ocean', '#65caff'],
  amethyst: ['Amethyst', '#c6a0ff'],
  forest: ['Forest', '#7bd6a1'],
  rose: ['Rose', '#f7a0c1'],
  amber: ['Amber', '#eec16b'],
};
const shortcutGroups = [
  ['Add files', 'O'],
  ['Export track', 'E'],
  ['Export queue', 'Shift+E'],
  ['Undo / redo', 'Z / Shift+Z'],
  ['Bypass effects', 'B'],
  ['Stop', '.'],
  ['Play / pause', 'Space'],
  ['Cut / copy / paste', 'X / C / V'],
  ['Select all / keep selection', 'A / T'],
  ['Zoom in / out / fit', '+ / − / 0'],
];
const builtIns = [
  'Balanced',
  'AI Cleanup',
  'Vocal Boost',
  'Bass Boost',
  'Bright & Wide',
  'Warm Glue',
  'Club Loud',
];
function stored(key, fallback) {
  try {
    return JSON.parse(localStorage.getItem('faderwake.' + key)) ?? fallback;
  } catch {
    return fallback;
  }
}
function save(key, value) {
  localStorage.setItem('faderwake.' + key, JSON.stringify(value));
}
function makeButton(text, fn) {
  const b = document.createElement('button');
  b.textContent = text;
  b.onclick = fn;
  return b;
}
const isField = (el) =>
  !!el?.closest(
    'input[type=text],input[type=number],input:not([type]),textarea,[contenteditable=true]',
  );

export function mountWorkspace(bridge) {
  let saved = stored('presets', []),
    appearance = stored('appearance', 'dark'),
    palette = stored('palette', 'studio');
  if (!Array.isArray(saved)) saved = [];
  const appearanceQuery = matchMedia('(prefers-color-scheme: dark)');
  const paletteSelect = document.createElement('select'),
    appearanceSelect = document.createElement('select');
  let previousQueue = null,
    previousQueueState = '',
    previousSaved = null,
    builtInKeys = new Map(),
    customKeys = new Map(),
    exporting = false,
    localHistory = [],
    localRedo = [],
    lastNativeToken = null;
  let nativeAppearance;
  const builtInPresets = new Map();
  function applyTheme() {
    const nativeWindow = window.__TAURI__?.window?.getCurrentWindow?.();
    if (nativeWindow && nativeAppearance !== appearance) {
      nativeAppearance = appearance;
      nativeWindow.setTheme(appearance === 'system' ? null : appearance).catch((error) => {
        nativeAppearance = undefined;
        bridge.notify(String(error));
      });
    }
    const color = palettes[palette]?.[1] || palettes.studio[1];
    document.documentElement.dataset.appearance =
      appearance === 'system' ? (appearanceQuery.matches ? 'dark' : 'light') : appearance;
    document.documentElement.dataset.palette = palette;
    document.documentElement.style.setProperty('--amber', color);
    document.documentElement.style.setProperty('--teal', palette === 'studio' ? '#a9dcd1' : color);
    bridge.redraw();
  }
  for (const sel of [paletteSelect, appearanceSelect]) {
    const isPalette = sel === paletteSelect,
      label = document.createElement('label');
    label.textContent = isPalette ? 'Color palette' : 'Appearance';
    label.append(sel);
    const options = isPalette
      ? Object.entries(palettes).map(([k, v]) => [k, v[0]])
      : [
          ['system', 'Follow system'],
          ['dark', 'Dark'],
          ['light', 'Light'],
        ];
    for (const [key, name] of options) sel.add(new Option(name, key));
    sel.value = isPalette ? palette : appearance;
    sel.onchange = () => {
      if (isPalette) palette = sel.value;
      else appearance = sel.value;
      save(isPalette ? 'palette' : 'appearance', sel.value);
      applyTheme();
    };
    $('theme-controls').append(label);
  }
  appearanceQuery.addEventListener('change', applyTheme);
  const modifier = /Mac/i.test(navigator.platform) ? '⌘' : 'Ctrl+';
  for (const [label, key] of shortcutGroups) {
    const dt = document.createElement('dt'),
      dd = document.createElement('dd');
    dt.textContent = label;
    dd.textContent = key === 'Space' ? 'Space' : modifier + key;
    $('shortcut-list').append(dt, dd);
  }
  $('appearance').onclick = () => $('preferences-dialog').showModal();
  for (const b of document.querySelectorAll('.dialog-close'))
    b.onclick = () => b.closest('dialog').close();
  function closeQueue() {
    document.body.classList.remove('queue-open');
    $('toggle-queue').setAttribute('aria-expanded', 'false');
  }
  $('toggle-queue').onclick = () => {
    const v = document.body.classList.toggle('queue-open');
    $('toggle-queue').setAttribute('aria-expanded', String(v));
  };
  for (const b of document.querySelectorAll('[data-tab]'))
    b.onclick = () => bridge.setTab(b.dataset.tab);
  for (const id of ['queue-add', 'start-open']) $(id).onclick = () => bridge.load(false);
  $('start-demo').onclick = () => bridge.load(true);
  const run = async (fn) => {
    try {
      await fn();
    } catch (e) {
      bridge.notify(String(e));
    }
  };
  $('remove').onclick = () =>
    run(() => bridge.mutate('remove_track', { trackId: selectedQueueId() }));
  $('reveal').onclick = () =>
    run(() => bridge.invoke('reveal_track', { trackId: selectedQueueId() }));
  $('reveal-export').onclick = () => run(() => bridge.invoke('reveal_export'));
  function selectedQueueId() {
    const s = bridge.getSnapshot();
    return s.queue.find((r) => r.selected)?.id ?? s.track?.id;
  }
  const defaultOutput = { format: 'wav', sampleRate: 44100, bitDepth: 24 };
  function fingerprint(settings, output) {
    const sorted = (value) =>
      Array.isArray(value)
        ? value.map(sorted)
        : value && typeof value === 'object'
          ? Object.fromEntries(
              Object.keys(value)
                .sort()
                .map((key) => [key, sorted(value[key])]),
            )
          : value;
    return JSON.stringify(sorted([settings, output]));
  }
  function matchPreset(settings, output) {
    if (!settings || !output) return null;
    const key = fingerprint(settings, output);
    return customKeys.get(key) || builtInKeys.get(key) || null;
  }
  async function loadPresets() {
    const result = await Promise.all(
      builtIns.map(async (name) => {
        const settings = await bridge.invoke('get_preset_settings', { preset: name });
        builtInPresets.set(name, { settings, output: defaultOutput });
        return [fingerprint(settings, defaultOutput), name];
      }),
    );
    // In case two presets have identical settings, the first built-in name wins.
    builtInKeys = new Map(result.reverse());
    if (onMac && !stored('legacy-presets-imported', false)) {
      try {
        await importLegacy();
      } catch (error) {
        bridge.notify('Could not import saved presets: ' + String(error));
      }
    }
  }
  const onMac = /Mac/i.test(navigator.platform);
  $('import-legacy').hidden = !onMac;
  async function importLegacy() {
    const presets = await bridge.invoke('import_legacy_presets');
    if (!Array.isArray(presets)) throw Error('Unexpected preset data.');
    const names = new Set(
      [...builtIns, ...saved.map((item) => item.name)].map((name) => name.toLowerCase()),
    );
    const additions = [];
    for (const preset of presets) {
      const name = preset.name?.trim();
      if (!name || !preset.settings || !preset.output || names.has(name.toLowerCase())) continue;
      names.add(name.toLowerCase());
      additions.push({ ...preset, name });
    }
    if (additions.length) restorePresets([...saved, ...additions]);
    save('legacy-presets-imported', true);
    return additions.length;
  }
  $('import-legacy').onclick = async () => {
    const b = $('import-legacy');
    b.disabled = true;
    try {
      const count = await importLegacy();
      presetNote(
        count
          ? 'Imported ' + count + ' saved ' + (count === 1 ? 'preset.' : 'presets.')
          : 'No new saved presets found.',
      );
    } catch (error) {
      presetNote(String(error));
    } finally {
      b.disabled = false;
    }
  };
  function renderQueue(s) {
    const signature = String(s.status.playing) + '/' + s.track?.id;
    if (previousQueue === s.queue && signature === previousQueueState && previousSaved === saved)
      return;
    previousQueue = s.queue;
    previousQueueState = signature;
    previousSaved = saved;
    $('queue-count').textContent = s.queue.length;
    $('queue-items').replaceChildren(
      ...s.queue.map((item, i) => {
        const row = document.createElement('button');
        row.className = 'queue-track';
        row.dataset.trackId = item.id;
        row.setAttribute('aria-pressed', String(item.selected));
        const slot = document.createElement('span');
        slot.className = 'queue-index';
        slot.textContent = item.error
          ? '!'
          : item.selected && s.status.playing
            ? '▶'
            : String(i + 1).padStart(2, '0');
        const text = document.createElement('span'),
          name = document.createElement('strong'),
          detail = document.createElement('small');
        name.textContent = item.name;
        detail.textContent = [
          item.settings ? matchPreset(item.settings, item.output) || 'Custom' : item.preset,
          item.edited ? 'Edited' : '',
          item.exportStatus && item.exportStatus.toLowerCase() !== 'ready' ? item.exportStatus : '',
        ]
          .filter(Boolean)
          .join(' · ');
        text.append(name, detail);
        row.append(slot, text);
        row.title = item.error || item.name;
        row.onclick = () =>
          run(async () => {
            await bridge.mutate('select_track', { trackId: item.id });
            closeQueue();
          });
        row.oncontextmenu = (event) => {
          event.preventDefault();
          showTrackMenu(item.id, item.name);
        };
        return row;
      }),
    );
  }
  const trackMenu = document.createElement('dialog');
  trackMenu.className = 'track-dialog';
  trackMenu.setAttribute('aria-labelledby', 'queue-action-title');
  document.body.append(trackMenu);
  function showTrackMenu(id, name) {
    const title = document.createElement('h2');
    title.id = 'queue-action-title';
    title.textContent = name;
    const preset = document.createElement('select');
    for (const value of [...builtIns, ...saved.map((p) => p.name)])
      preset.add(new Option(value, value));
    preset.setAttribute('aria-label', 'Chain preset');
    const action = (label, task) =>
      makeButton(label, () => {
        trackMenu.close();
        run(task);
      });
    trackMenu.replaceChildren(
      title,
      action('Load track', () => bridge.mutate('select_track', { trackId: id })),
      preset,
      action('Apply preset', () => applyPreset(preset.value, id)),
      action('Show in file manager', () => bridge.invoke('reveal_track', { trackId: id })),
      action('Remove from queue', () => bridge.mutate('remove_track', { trackId: id })),
      makeButton('Close', () => trackMenu.close()),
    );
    trackMenu.showModal();
  }
  function renderSaved() {
    customKeys = new Map(
      [...saved].reverse().map((p) => [fingerprint(p.settings, p.output), p.name]),
    );
    $('saved-presets').replaceChildren(...saved.map((item) => new Option(item.name, item.name)));
    for (const id of ['saved-presets', 'apply-saved', 'delete-saved'])
      $(id).disabled = !saved.length;
    for (const option of $('preset').querySelectorAll('option[data-saved]')) option.remove();
    for (const item of saved) {
      const option = new Option(item.name, item.name);
      option.dataset.saved = '';
      $('preset').add(option);
    }
  }
  function recordLocal(before, after) {
    localHistory.push({ before, after });
    localRedo = [];
    lastNativeToken = null;
  }
  function recordNative(token) {
    if (token !== undefined && token === lastNativeToken) return;
    localHistory.push({ native: true });
    localRedo = [];
    lastNativeToken = token;
  }
  function restorePresets(list) {
    saved = structuredClone(list);
    save('presets', saved);
    renderSaved();
    bridge.updatePreset?.();
  }
  function changePresets(next) {
    recordLocal(structuredClone(saved), structuredClone(next));
    restorePresets(next);
  }
  function localCan(redo) {
    const stack = redo ? localRedo : localHistory;
    return !!stack.length && !stack.at(-1).native;
  }
  async function history(redo) {
    if (bridge.getSnapshot().tab === 'editor') {
      return bridge.tabAction(redo ? 'redo' : 'undo');
    }
    await bridge.flush();
    const from = redo ? localRedo : localHistory,
      to = redo ? localHistory : localRedo;
    const last = from.at(-1);
    if (last && !last.native) restorePresets(redo ? last.after : last.before);
    else await bridge.mutate('workspace_history', { redo });
    if (last) {
      from.pop();
      to.push(last);
    }
    lastNativeToken = null;
    render(bridge.getSnapshot());
  }
  $('undo').onclick = () => run(() => history(false));
  $('redo').onclick = () => run(() => history(true));
  $('manage-presets').onclick = () => {
    renderSaved();
    $('preset-name').value = '';
    $('presets-dialog').querySelector('.dialog-note').textContent = '';
    $('presets-dialog').showModal();
  };
  function presetNote(message) {
    $('presets-dialog').querySelector('.dialog-note').textContent = message;
  }
  $('save-preset').onclick = () => {
    const { track, settings } = bridge.getSnapshot();
    const name = $('preset-name').value.trim();
    if (!track) return;
    if (
      !name ||
      builtIns.some((v) => v.toLowerCase() === name.toLowerCase()) ||
      name === 'edited'
    ) {
      presetNote('Choose a unique preset name different from the built-in chains.');
      return;
    }
    const index = saved.findIndex((p) => p.name.toLowerCase() === name.toLowerCase());
    const item = {
      name,
      settings: structuredClone(settings),
      output: structuredClone(track.output),
    };
    const next = saved.slice();
    if (index < 0) next.push(item);
    else next[index] = item;
    changePresets(next);
    $('saved-presets').value = name;
    presetNote('Saved ' + name + '.');
  };
  $('delete-saved').onclick = () => {
    const name = $('saved-presets').value;
    if (name) {
      changePresets(saved.filter((p) => p.name !== name));
      presetNote('Deleted ' + name + '.');
    }
  };
  $('apply-saved').onclick = () =>
    run(async () => {
      await applyPreset($('saved-presets').value);
      $('presets-dialog').close();
    });
  $('reset-chain').onclick = () =>
    run(async () => {
      await applyPreset('Balanced');
      $('presets-dialog').close();
    });
  $('apply-queue').onclick = () =>
    run(async () => {
      await bridge.flush();
      const { track } = bridge.getSnapshot();
      await bridge.mutate('apply_to_all', { trackId: track.id, what: 'settings' });
      $('presets-dialog').close();
    });
  async function applyPreset(name, id = bridge.getSnapshot().track?.id) {
    if (id === undefined) return;
    const preset = saved.find((p) => p.name === name) || builtInPresets.get(name);
    await bridge.applyPreset(name, preset, id);
  }
  function openExport(all = false) {
    const s = bridge.getSnapshot();
    if ((all ? !s.queue.length : !s.track) || exporting) return;
    $('export-scope').options[0].disabled = !s.track;
    $('export-scope').options[1].disabled = !s.queue.length;
    $('export-scope').value = all && s.queue.length ? 'all' : 'one';
    exportDetails();
    $('export-dialog').showModal();
  }
  function exportDetails() {
    const { track, queue } = bridge.getSnapshot();
    if (!track && !queue.length) return;
    const all = $('export-scope').value === 'all',
      rows = all ? queue : [track];
    $('export-summary').replaceChildren(
      ...rows.map((row) => {
        const line = document.createElement('p'),
          value = row.output;
        line.textContent =
          row.name +
          ' · ' +
          value.format.toUpperCase() +
          ' · ' +
          value.sampleRate / 1000 +
          ' kHz' +
          (value.format !== 'aac' ? ' · ' + value.bitDepth + ' bit' : '');
        return line;
      }),
    );
  }
  $('export').onclick = () => openExport();
  $('export-scope').onchange = exportDetails;
  $('confirm-export').onclick = () =>
    run(async () => {
      const all = $('export-scope').value === 'all';
      $('export-dialog').close();
      await bridge.flush();
      if (exporting) return;
      exporting = true;
      render(bridge.getSnapshot());
      try {
        const result = await bridge.invoke('export_audio', { all });
        if (result) bridge.notify('Exported ' + result);
      } finally {
        exporting = false;
        await bridge.refresh();
      }
    });
  $('cancel-export').onclick = () => run(() => bridge.invoke('cancel_export'));
  const editorActions = [
    'cut',
    'copy',
    'paste',
    'select-all',
    'trim',
    'fade-in',
    'fade-out',
    'silence',
    'reverse',
    'zoom-in',
    'zoom-out',
    'fit-track',
    'zoom-selection',
  ];
  async function action(name) {
    const s = bridge.getSnapshot(),
      focus = document.activeElement;
    if (isField(focus) && ['undo', 'redo', 'copy', 'cut', 'paste', 'select-all'].includes(name)) {
      const command = name === 'select-all' ? 'selectAll' : name;
      if (!document.execCommand(command) && name === 'paste' && navigator.clipboard) {
        const text = await navigator.clipboard.readText();
        if (document.activeElement === focus) document.execCommand('insertText', false, text);
      }
      return;
    }
    if (document.querySelector('dialog[open]')) return;
    if (editorActions.includes(name)) {
      if (s.tab === 'editor') await bridge.tabAction(name);
      return;
    }
    switch (name) {
      case 'settings':
        $('preferences-dialog').showModal();
        break;
      case 'demo':
      case 'open':
        await bridge.load(name === 'demo');
        break;
      case 'export':
      case 'export-all':
        openExport(name === 'export-all');
        break;
      case 'undo':
      case 'redo':
        await history(name === 'redo');
        break;
      case 'play':
        await bridge.togglePlaying();
        break;
      case 'stop':
        await bridge.stop();
        break;
      case 'bypass':
        if (s.track) await bridge.audition(!s.status.original, s.status.matched);
        break;
    }
  }
  const keyActions = {
    o: 'open',
    e: 'export',
    b: 'bypass',
    '.': 'stop',
    ',': 'settings',
    z: 'undo',
    y: 'redo',
  };
  document.addEventListener('keydown', (e) => {
    if (e.defaultPrevented || e.altKey || document.querySelector('dialog[open]')) return;
    if (e.key === 'Escape') {
      closeQueue();
      return;
    }
    const s = bridge.getSnapshot();
    const key = e.key.toLowerCase();
    if (!(e.metaKey || e.ctrlKey)) {
      if (
        (key === 'delete' || key === 'backspace') &&
        !isField(e.target) &&
        e.target.closest('#queue-panel')
      ) {
        e.preventDefault();
        const row = e.target.closest('.queue-track'),
          id = row ? Number(row.dataset.trackId) : selectedQueueId();
        if (id && !s.status.exporting) run(() => bridge.mutate('remove_track', { trackId: id }));
      }
      return;
    }
    if (window.__TAURI__?.event && ['o', 'e', 'b', '.', ',', 'z', ' '].includes(key)) return;
    if (isField(e.target) && ['z', 'y'].includes(key)) return;
    if (s.tab === 'editor' && ['z', 'y'].includes(key)) return;
    let name = keyActions[key];
    if (key === 'z' && e.shiftKey) name = 'redo';
    if (key === 'e' && e.shiftKey) name = 'export-all';
    if (key === ' ') name = 'play';
    if (name) {
      e.preventDefault();
      run(() => action(name));
    }
  });
  let previousMenu = '',
    menuTimer = 0;
  function syncMenu() {
    if (!window.__TAURI__?.event) return;
    const s = bridge.getSnapshot(),
      field = isField(document.activeElement),
      has = !!s.track,
      edit = s.tab === 'editor' && has;
    const enabled = {
      open: !$('open').disabled,
      demo: !$('demo').disabled,
      export: !$('export').disabled,
      'export-all': s.queue.length > 0 && !s.status.exporting,
      undo: field || !$('undo').disabled,
      redo: field || !$('redo').disabled,
      play: has,
      stop: has,
      bypass: has,
      settings: true,
    };
    for (const name of editorActions) {
      const cmd = { 'fade-in': 'fadeIn', 'fade-out': 'fadeOut' }[name] || name;
      const button = $('editor-pane').querySelector('[data-cmd="' + cmd + '"]');
      enabled[name] = edit && !!button && !button.disabled;
    }
    if (field) for (const name of ['cut', 'copy', 'paste', 'select-all']) enabled[name] = true;
    const state = { enabled, editingText: field },
      key = JSON.stringify(state);
    if (key === previousMenu) return;
    previousMenu = key;
    clearTimeout(menuTimer);
    menuTimer = setTimeout(
      () => bridge.invoke('set_menu_state', state).catch((e) => bridge.notify(String(e))),
      30,
    );
  }
  document.addEventListener('focusin', syncMenu);
  document.addEventListener('focusout', () => queueMicrotask(syncMenu));
  function render(s) {
    renderQueue(s);
    const has = !!s.track,
      exp = exporting || s.status.exporting;
    $('welcome').hidden = has;
    $('desk').hidden = !has;
    $('reveal').disabled = !selectedQueueId();
    $('remove').disabled = !selectedQueueId() || exp;
    $('manage-presets').disabled = !has;
    $('apply-queue').disabled = s.queue.length < 2 || !has;
    $('undo').disabled =
      s.tab === 'editor' ? !s.status.canUndo : !s.status.canUndoWorkspace && !localCan(false);
    $('redo').disabled =
      s.tab === 'editor' ? !s.status.canRedo : !s.status.canRedoWorkspace && !localCan(true);
    $('reveal-export').hidden = !s.status.lastExport;
    $('export-progress').hidden = !exp;
    const progress = $('export-progress').querySelector('progress');
    progress.value = s.status.exportProgress || 0;
    $('export-progress').querySelector('span').textContent =
      'Exporting · ' + Math.round(progress.value * 100) + '%';
    $('export').disabled ||= exp;
    const failed = !has && s.queue.find((row) => row.error),
      welcome = $('welcome');
    welcome.querySelector('h1').textContent = failed
      ? 'Couldn’t open this track'
      : s.queue.length && !has
        ? 'Select a track'
        : 'Add your first track';
    welcome.querySelector('p').textContent = failed
      ? failed.error
      : s.queue.length && !has
        ? 'Choose a track from the queue.'
        : 'Drop audio files here to start mastering.';
    $('retry-track').hidden = !failed;
    if (failed)
      $('retry-track').onclick = () =>
        run(() => bridge.mutate('select_track', { trackId: failed.id }));
    syncMenu();
  }
  renderSaved();
  return {
    render,
    applyTheme,
    applyPreset,
    recordNative,
    action,
    closeQueue,
    matchPreset,
    loadPresets,
  };
}
