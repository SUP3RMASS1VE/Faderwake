// WKWebView executes this function body in the packaged app's page content world.
const $ = (id) => document.getElementById(id);
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const wait = async (condition, description) => {
    const start = Date.now();
    while (!(await condition())) {
        if (Date.now() - start > 50000) throw Error("Timed out waiting for " + description);
        await sleep(80);
    }
};
await wait(() => document.querySelectorAll('#theme-controls select').length === 2, 'application startup');
const invoke = window.__TAURI__.core.invoke;
if (!$('snapshot-motion')) {
    const style = document.createElement('style');
    style.id = 'snapshot-motion';
    style.textContent = '* { transition: none !important; animation: none !important; }';
    document.head.append(style);
}
const [palette, appearance] = document.querySelectorAll('#theme-controls select');
function choose(control, value) {
    control.value = value;
    if (control.value !== value) throw Error('Unavailable control value: ' + value);
    control.dispatchEvent(new Event('change', {bubbles: true}));
}
if ($('preferences-dialog').open) $('preferences-dialog').close();
choose(palette, args.palette);
choose(appearance, args.mode);
if (args.glass) {
    const state = await invoke('get_glass_availability');
    if (!state.supported || state.reduceTransparency) throw Error('Liquid Glass requires an available native material and Reduce Transparency off.');
    if (!$('liquid-glass').checked) $('liquid-glass').click();
    await wait(() => document.documentElement.hasAttribute('data-liquid-glass'), 'native Liquid Glass');
}
if (args.page === 'empty') {
    if ($('welcome').hidden || (await invoke('get_queue')).length) throw Error('Expected an empty studio.');
} else {
    await wait(async () => {
        const state = await invoke('get_status');
        if (state.error) throw Error(state.error);
        return !state.rendering && state.masterLufs != null;
    }, 'master render');
    await window.__TAURI__.event.emit('queue-changed', null);
    await wait(() => $('queue-count').textContent === '2' && $('welcome').hidden, 'populated queue');
    const editor = ['editor', 'fade-in', 'fade-out'].includes(args.page);
    const tab = editor ? 'editor' : args.page === 'metadata' ? 'metadata' : 'master';
    document.querySelector('[data-tab="' + tab + '"]').click();
    if (tab !== 'editor') {
        $('visual').value = args.page === 'spectrum' ? 'spectrogram' : 'combined';
        await $('visual').onchange();
    }
    if (tab === 'master') {
        $(args.page === 'dynamics' ? 'dynamics-section' : 'tone-section').click();
        await wait(() => $('wave').width > 0 && !$('desk').hidden, 'master waveform');
    } else if (editor) {
        await wait(() => !$('editor-pane').hidden && Number(document.querySelector('#editor-pane [data-id="end"]').value) === 6, 'editor selection');
    } else {
        await wait(() => !$('metadata-pane').hidden && [...$('metadata-pane').querySelectorAll('input')].some(el => el.value === 'Half Light'), 'metadata fields');
    }
    if (args.page === 'preferences') $('appearance').click();
}
await document.fonts.ready;
window.scrollTo(0,0);
await sleep(220);
if (document.documentElement.scrollWidth > innerWidth + 1) throw Error('Horizontal page overflow');
for (const selector of ['.app-bar', '.source', '.meter-panel', '.equalizer', '.inspector', '#polish', '#output-strip']) {
    const element = document.querySelector(selector);
    if (element?.getClientRects().length && element.scrollWidth > element.clientWidth + 2) {
        throw Error('Clipped content in ' + selector);
    }
}
if (args.page === 'master' && innerWidth >= 1200 && innerHeight >= 820 && $('output-strip').getBoundingClientRect().bottom > innerHeight) {
    const sizes = ['.source', '.equalizer', '#polish', '#output-strip'].map(selector => {
        const rect = document.querySelector(selector).getBoundingClientRect();
        return selector + '=' + Math.round(rect.height) + '@' + Math.round(rect.top);
    });
    throw Error('Master controls exceed viewport ' + innerHeight + ': ' + sizes.join(', '));
}
const exportLabel = document.createRange();
exportLabel.selectNodeContents($('export'));
if (exportLabel.getBoundingClientRect().width > $('export').clientWidth) throw Error('Clipped Export label');
return true;
