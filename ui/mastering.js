const $ = (id) => document.getElementById(id);
const polishStages = [
  ['cleanLowEnd', 'Clean low end'],
  ['centerBass', 'Center bass'],
  ['cutMud', 'Cut mud'],
  ['tameHarsh', 'Tame harshness'],
  ['addAir', 'Add air'],
  ['smoothHighs', 'De-shimmer AI highs'],
];
const sections = [
  {
    title: 'Loudness & stereo',
    fields: [
      ['inputGainDb', 'Input gain', -12, 12, 0.1, 'dB'],
      ['normalize', 'Normalize loudness'],
      ['targetLufs', 'Target loudness', -20, -6, 0.1, 'LUFS', 'normalize'],
      ['limit', 'Limit true peak'],
      ['ceilingDb', 'Peak ceiling', -3, 0, 0.1, 'dBTP', 'limit'],
      ['width', 'Stereo width', 0, 200, 1, '%'],
    ],
  },
  {
    title: 'Compressor',
    toggle: 'glue',
    fields: [
      ['compressor.threshold', 'Threshold', -48, 0, 0.1, 'dB'],
      ['compressor.ratio', 'Ratio', 1, 10, 0.1, ':1'],
      ['compressor.attack', 'Attack', 0.1, 100, 0.1, 'ms'],
      ['compressor.release', 'Release', 10, 1000, 1, 'ms'],
      ['compressor.makeup', 'Makeup gain', 0, 12, 0.1, 'dB'],
    ],
  },
  {
    title: 'De-esser',
    toggle: 'deEsser',
    fields: [
      ['deEsser.frequency', 'Frequency', 2000, 10000, 1, 'Hz'],
      ['deEsser.threshold', 'Threshold', -48, 0, 0.1, 'dB'],
      ['deEsser.reduction', 'Maximum cut', 0, 12, 0.1, 'dB'],
    ],
  },
];
const defaultDeEsser = { frequency: 5500, threshold: -24, reduction: 6 };
const outputFormats = { wav: 'WAV', flac: 'FLAC', alac: 'Apple Lossless', aac: 'AAC' };
const outputRates = [44100, 48000, 88200, 96000, 176400, 192000];
const streamingTargets = [
  ['Spotify', -14, -1],
  ['Apple Music', -16, -1],
  ['YouTube', -14, -1],
  ['Amazon Music', -14, -2],
  ['Tidal', -14, -1],
  ['Deezer', -15, -1],
];
const element = (tag, text, className) => {
  const node = document.createElement(tag);
  if (text) node.textContent = text;
  if (className) node.className = className;
  return node;
};
const getValue = (settings, path) => path.split('.').reduce((obj, key) => obj?.[key], settings);
const db = (value) => (Number.isFinite(value) ? value.toFixed(1) : '—');

export function mountMastering(bridge) {
  const chips = new Map(),
    checks = new Map(),
    values = new Map(),
    outputControls = {};
  const dynamics = $('dynamics');
  let section = 'tone',
    previousSettings = '',
    previousOutput = '',
    previousMetrics = '';
  const updateSetting = (key, value, instant = true) => {
    const settings = bridge.getSnapshot().settings;
    if (!settings) return;
    const [group, prop] = key.split('.');
    if (prop) settings[group][prop] = value;
    else settings[group] = key === 'deEsser' ? (value ? { ...defaultDeEsser } : null) : value;
    bridge.edit(instant);
  };
  function switchSection(value) {
    section = value;
    const show = bridge.getSnapshot().tab === 'master';
    $('tone-section').setAttribute('aria-pressed', String(value === 'tone'));
    $('dynamics-section').setAttribute('aria-pressed', String(value === 'dynamics'));
    for (const selector of ['.equalizer', '.inspector', '#polish'])
      document.querySelector(selector).hidden = !show || value !== 'tone';
    dynamics.hidden = !show || value !== 'dynamics';
    if (show && value === 'tone') bridge.drawGraph();
  }
  $('tone-section').onclick = () => switchSection('tone');
  $('dynamics-section').onclick = () => switchSection('dynamics');
  for (const [key, title] of polishStages) {
    const button = element('button', title);
    button.onclick = () => updateSetting(key, !getValue(bridge.getSnapshot().settings, key));
    chips.set(key, button);
    $('polish-chips').append(button);
  }
  function check(key, label, parent) {
    const root = element('label', null, 'master-check'),
      input = element('input');
    input.type = 'checkbox';
    input.onchange = () => updateSetting(key, input.checked);
    root.append(input, document.createTextNode(label));
    parent.append(root);
    checks.set(key, input);
  }
  function addField(card, field, parentToggle) {
    const [key, label, min, max, step, unit, condition] = field;
    if (min === undefined) {
      check(key, label, card);
      return;
    }
    const row = element('div', null, 'dynamic-field'),
      title = element('label', label),
      slider = element('input'),
      number = element('input');
    const numericId = 'parameter-' + key.replace('.', '-');
    title.htmlFor = numericId;
    number.id = numericId;
    const readout = element('span', null, 'numeric-group');
    readout.append(number, element('small', unit));
    number.type = 'number';
    slider.type = 'range';
    slider.setAttribute('aria-label', label + ' slider');
    for (const input of [number, slider]) {
      input.min = min;
      input.max = max;
      input.step = step;
    }
    number.onfocus = () => (number.dataset.trackId = String(bridge.getSnapshot().track?.id));
    number.onchange = () => {
      if (
        number.dataset.trackId &&
        number.dataset.trackId !== String(bridge.getSnapshot().track?.id)
      ) {
        number.value = getValue(bridge.getSnapshot().settings, key) ?? '';
        return;
      }
      const value = number.valueAsNumber;
      if (Number.isFinite(value)) {
        updateSetting(key, Math.min(max, Math.max(min, value)));
      } else number.value = getValue(bridge.getSnapshot().settings, key) ?? '';
    };
    number.onkeydown = (e) => {
      if (e.key === 'Escape') {
        number.value = getValue(bridge.getSnapshot().settings, key);
        number.blur();
      }
      if (e.key === 'Enter') number.blur();
    };
    slider.onpointerdown = bridge.beginGesture;
    slider.onpointerup = bridge.endGesture;
    slider.onpointercancel = bridge.endGesture;
    slider.onchange = bridge.flush;
    slider.oninput = () => {
      number.value = slider.value;
      updateSetting(key, Number(slider.value), false);
    };
    row.append(title, readout, slider);
    card.append(row);
    values.set(key, { number, slider, required: condition || parentToggle, min, max });
  }
  for (const group of sections) {
    const card = element('section', null, 'panel dynamic-card');
    if (group.toggle) check(group.toggle, group.title, card);
    else card.append(element('h2', group.title));
    for (const f of group.fields) addField(card, f, group.toggle);
    if (!group.toggle) {
      const select = element('select');
      select.setAttribute('aria-label', 'Streaming loudness target');
      select.add(new Option('Streaming target…', ''));
      for (const [name, target, peak] of streamingTargets)
        select.add(new Option(name + ' (' + target + ' LUFS, ' + peak + ' dBTP)', name));
      select.onchange = () => {
        const choice = streamingTargets.find((row) => row[0] === select.value);
        if (!choice) return;
        Object.assign(bridge.getSnapshot().settings, {
          normalize: true,
          limit: true,
          targetLufs: choice[1],
          ceilingDb: choice[2],
        });
        bridge.edit(true);
        select.value = '';
      };
      card.append(select);
    }
    dynamics.append(card);
  }
  const compare = element('section', null, 'panel dynamic-card compare');
  compare.append(element('h2', 'Integrated file loudness'));
  const rows = [];
  for (const name of ['Original', 'Master']) {
    const row = element('div', null, 'compare-row'),
      bar = element('meter'),
      val = element('b', '—');
    bar.min = -60;
    bar.max = 0;
    bar.setAttribute('aria-label', name + ' integrated loudness');
    row.append(element('span', name), bar, val);
    compare.append(row);
    rows.push([bar, val]);
  }
  const difference = element('p');
  compare.append(difference);
  dynamics.append(compare);
  for (const [key, label] of [
    ['format', 'Format'],
    ['sampleRate', 'Sample rate'],
    ['bitDepth', 'Bit depth'],
  ]) {
    const wrap = element('label', label),
      select = element('select');
    select.setAttribute('aria-label', 'Output ' + label.toLowerCase());
    if (key === 'format')
      for (const [k, v] of Object.entries(outputFormats)) select.add(new Option(v, k));
    if (key === 'bitDepth')
      for (const bits of [16, 24]) select.add(new Option(bits + ' bit', bits));
    select.onchange = async () => {
      const current = bridge.getSnapshot().track;
      if (!current) return;
      const output = {
        ...current.output,
        [key]: key === 'format' ? select.value : Number(select.value),
      };
      if (output.format === 'aac' && ![44100, 48000].includes(output.sampleRate))
        output.sampleRate = 48000;
      try {
        await bridge.setOutput(output);
      } catch (err) {
        bridge.notify(String(err));
      }
    };
    wrap.append(select);
    $('output-fields').append(wrap);
    outputControls[key] = select;
  }
  function update(snapshot, force = false) {
    const { settings, track, status } = snapshot;
    if (!settings || !track) return;
    const encoded = JSON.stringify(settings);
    if (force || encoded !== previousSettings) {
      previousSettings = encoded;
      for (const [key, btn] of chips) btn.setAttribute('aria-pressed', String(!!settings[key]));
      for (const [key, input] of checks) input.checked = !!settings[key];
      for (const [key, { number, slider, required, min, max }] of values) {
        const value = getValue(settings, key);
        if (document.activeElement !== number) number.value = value ?? '';
        slider.value = value ?? min;
        slider.style.setProperty('--fraction', (100 * ((value ?? min) - min)) / (max - min) + '%');
        number.disabled = slider.disabled = !!required && !settings[required];
      }
      $('enable-eq').checked = !settings.eqBypassed;
    }
    const output = track.output;
    if (output && (force || JSON.stringify(output) !== previousOutput)) {
      previousOutput = JSON.stringify(output);
      const permitted = output.format === 'aac' ? outputRates.slice(0, 2) : outputRates;
      outputControls.sampleRate.replaceChildren(
        ...permitted.map((r) => new Option(r / 1000 + ' kHz', r)),
      );
      for (const key of Object.keys(outputControls)) outputControls[key].value = output[key];
      outputControls.bitDepth.disabled = output.format === 'aac';
    }
    const metrics = [
      status.originalLufs ?? track.originalLufs,
      status.masterLufs,
      settings.normalize ? settings.targetLufs : null,
    ];
    const signature = metrics.join('/');
    if (force || signature !== previousMetrics) {
      previousMetrics = signature;
      rows.forEach(([bar, value], i) => {
        bar.value = Number.isFinite(metrics[i]) ? metrics[i] : -60;
        value.textContent = db(metrics[i]) + ' LUFS';
      });
      const gain = metrics[1] - metrics[0];
      const delta = metrics.slice(0, 2).every(Number.isFinite)
        ? (gain > 0 ? '+' : '') + db(gain) + ' LU change'
        : '';
      difference.textContent = [
        delta,
        settings.normalize ? 'Target ' + settings.targetLufs + ' LUFS' : '',
      ]
        .filter(Boolean)
        .join(' · ');
    }
  }
  return { update, layout: () => switchSection(section) };
}
