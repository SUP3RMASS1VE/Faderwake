// AppKit owns the material. The web layer paints the workspace around its native surfaces.
export function mountGlass(bridge) {
  if (!window.__TAURI__?.core) return;
  const root = document.documentElement;
  const control = document.getElementById('liquid-glass');
  const help = document.getElementById('glass-description');
  const toolbar = document.querySelector('.app-bar');
  const sidebar = document.getElementById('queue-panel');
  const preferenceKey = 'faderwake.liquidGlass';
  let wanted = localStorage.getItem(preferenceKey) === 'true';
  let available = false;
  let reduced = false;
  let nativeActive = false;
  let frame = 0;
  let sending = false;
  let changed = false;
  let lastGeometry = '';

  function showAvailability(state) {
    available = state.supported;
    reduced = state.reduceTransparency;
    nativeActive = state.active;
    control.checked = wanted;
    control.disabled = !available || reduced;
    help.textContent = !available
      ? 'Available on macOS 26 and later.'
      : reduced
        ? 'Reduce Transparency is enabled in macOS Accessibility.'
        : 'Use Apple’s glass material for the toolbar and track queue.';
    document.getElementById('glass-preference').hidden = false;
  }

  function region(element, insetX, insetY, radius) {
    const rect = element.getBoundingClientRect();
    const round = (n) => Math.round(n * 2) / 2;
    const x = round(rect.x + insetX),
      y = round(rect.y + insetY);
    const width = round(rect.width - 2 * insetX),
      height = round(rect.height - 2 * insetY);
    if (
      width <= 0 ||
      height <= 0 ||
      x + width <= 0 ||
      y + height <= 0 ||
      x >= innerWidth ||
      y >= innerHeight
    )
      return null;
    return { x, y, width, height, radius: Math.min(radius, width / 2, height / 2) };
  }

  function roundedOutline({ x, y, width, height, radius: r }) {
    const right = x + width,
      bottom = y + height;
    return `M ${x + r} ${y} H ${right - r} Q ${right} ${y} ${right} ${y + r} V ${bottom - r}
      Q ${right} ${bottom} ${right - r} ${bottom} H ${x + r} Q ${x} ${bottom} ${x} ${bottom - r}
      V ${y + r} Q ${x} ${y} ${x + r} ${y} Z`.replace(/\s+/g, ' ');
  }

  function paint(active, surfaces) {
    if (active) {
      const paths = surfaces.map(({ shape }) => roundedOutline(shape));
      root.style.setProperty('--glass-surfaces', `path("${paths.join(' ')}")`);
    } else {
      root.style.removeProperty('--glass-surfaces');
    }
    root.toggleAttribute('data-liquid-glass', active);
    for (const element of [toolbar, sidebar]) {
      element.toggleAttribute(
        'data-native-glass',
        active && surfaces.some((s) => s.element === element),
      );
    }
  }

  async function sync() {
    frame = 0;
    changed = false;
    sending = true;
    try {
      const enabled = available && wanted && !reduced;
      const surfaces = [];
      if (enabled) {
        const bar = region(toolbar, 8, 3, 14);
        if (bar) surfaces.push({ element: toolbar, shape: bar });
        if (getComputedStyle(sidebar).position !== 'fixed') {
          const rail = region(
            sidebar,
            0,
            0,
            parseFloat(getComputedStyle(sidebar).borderRadius) || 8,
          );
          if (rail) surfaces.push({ element: sidebar, shape: rail });
        }
      }
      const regions = surfaces.map((surface) => surface.shape);
      const signature = JSON.stringify([enabled, innerWidth, innerHeight, regions]);
      if (signature === lastGeometry) return;
      const previouslyActive = root.hasAttribute('data-liquid-glass');
      if (!enabled && previouslyActive) paint(false, []);
      if (enabled || nativeActive || previouslyActive || !lastGeometry) {
        const state = await bridge.invoke('set_liquid_glass', { enabled, regions });
        showAvailability(state);
        paint(Boolean(state.active && wanted && !reduced), surfaces);
      }
      lastGeometry = signature;
    } catch (error) {
      paint(false, []);
      wanted = false;
      control.checked = false;
      localStorage.removeItem(preferenceKey);
      bridge.notify('Could not apply Liquid Glass. ' + String(error));
    } finally {
      sending = false;
      if (changed) schedule();
    }
  }

  function schedule() {
    changed = true;
    if (!sending && !frame) frame = requestAnimationFrame(sync);
  }

  async function refreshAvailability() {
    const state = await bridge.invoke('get_glass_availability');
    showAvailability(state);
    if (available && (wanted || root.hasAttribute('data-liquid-glass'))) {
      lastGeometry = '';
      schedule();
    }
  }

  control.addEventListener('change', () => {
    wanted = control.checked;
    localStorage.setItem(preferenceKey, String(wanted));
    schedule();
  });
  const updateLayout = () => {
    if (wanted && available && !reduced) schedule();
  };
  const observer = new ResizeObserver(updateLayout);
  for (const node of [toolbar, sidebar, document.body]) observer.observe(node);
  new MutationObserver(updateLayout).observe(document.body, {
    attributes: true,
    attributeFilter: ['class'],
  });
  window.addEventListener('resize', updateLayout);
  document.addEventListener('scroll', updateLayout, { capture: true, passive: true });
  window.__TAURI__.event
    ?.listen('glass-accessibility-changed', () =>
      refreshAvailability().catch((error) => bridge.notify(String(error))),
    )
    .catch((error) => bridge.notify(String(error)));
  refreshAvailability().catch((error) => bridge.notify(String(error)));
}
