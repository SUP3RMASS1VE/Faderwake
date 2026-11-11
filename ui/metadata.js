const fields = [
  ['title', 'Title'],
  ['artist', 'Artist'],
  ['album', 'Album'],
  ['genre', 'Genre'],
  ['year', 'Year'],
  ['track', 'Track number'],
  ['comment', 'Comment'],
];

export function mountMetadata(container, bridge) {
  const panel = document.createElement('section');
  panel.className = 'fw-tags';
  panel.innerHTML = `<div class="ed-top"><div><span class="ed-label">Metadata</span><h2>File tags</h2></div>
    <span class="tag-state" role="status"></span></div><p class="ed-help">These tags are embedded when you export.</p>
    <div class="ed-tag-grid">${fields.map(([key, label]) => `<label>${label}${key === 'comment' ? '<textarea rows="3" data-tag="comment"></textarea>' : '<input type="text" data-tag="' + key + '">'}</label>`).join('')}</div>
    <div class="ed-tag-footer"><button type="button" data-all>Apply tags to all tracks</button></div>`;
  container.append(panel);
  const inputs = [...panel.querySelectorAll('[data-tag]')],
    button = panel.querySelector('[data-all]'),
    status = panel.querySelector('.tag-state');
  let active = false,
    id = null,
    queueSize = 0,
    version = 0,
    writing = null,
    timer = 0;
  const unsaved = new Map();
  const drafts = new Map();
  async function flush() {
    clearTimeout(timer);
    if (writing) return writing;
    if (!unsaved.size) return;
    writing = (async () => {
      while (unsaved.size) {
        const job = unsaved.values().next().value;
        try {
          await bridge.invoke('set_tags', { trackId: job.id, tags: job.tags });
          if (unsaved.get(job.id) === job) unsaved.delete(job.id);
          const current = bridge.getSnapshot()?.track;
          if (current?.id === job.id) current.tags = { ...job.tags };
          if (drafts.get(job.id) === job) drafts.delete(job.id);
          if (id === job.id && version === job.version) status.textContent = 'Saved for export';
        } catch (error) {
          bridge.notify(String(error));
          if (id === job.id) status.textContent = 'Could not save tags';
          throw error;
        }
      }
    })().finally(() => {
      writing = null;
    });
    return writing;
  }
  function edit() {
    if (id == null) return;
    const tags = Object.fromEntries(inputs.map((input) => [input.dataset.tag, input.value]));
    const job = { id, tags, version: ++version };
    drafts.set(id, job);
    unsaved.set(id, job);
    clearTimeout(timer);
    timer = setTimeout(() => flush().catch(() => {}), 240);
    status.textContent = 'Saving…';
  }
  for (const input of inputs) {
    input.addEventListener('input', edit);
    input.addEventListener('change', () => flush().catch(() => {}));
  }
  button.onclick = async () => {
    const sourceId = id;
    if (sourceId == null || queueSize < 2) return;
    button.disabled = true;
    try {
      await flush();
      await bridge.invoke('apply_to_all', { trackId: sourceId, what: 'tags' });
      await bridge.refresh();
      bridge.notify('Tags applied to every track.');
    } catch (error) {
      bridge.notify(String(error));
    } finally {
      button.disabled = queueSize < 2;
    }
  };
  function update(snapshot) {
    const next = snapshot.track;
    queueSize = Array.isArray(snapshot.queue)
      ? snapshot.queue.length
      : (snapshot.queue?.tracks?.length ?? 0);
    if ((next?.id ?? null) !== id) {
      flush().catch(() => {});
      id = next?.id ?? null;
      status.textContent = '';
    }
    button.disabled = id == null || queueSize < 2;
    if (!active) return;
    const tags = drafts.get(id)?.tags ?? next?.tags ?? {};
    for (const input of inputs) {
      input.disabled = id == null;
      if (document.activeElement !== input && input.value !== (tags[input.dataset.tag] ?? ''))
        input.value = tags[input.dataset.tag] ?? '';
    }
  }
  return {
    update,
    activate(snapshot) {
      active = true;
      update(snapshot);
    },
    deactivate() {
      active = false;
      flush().catch(() => {});
    },
    flush,
  };
}
