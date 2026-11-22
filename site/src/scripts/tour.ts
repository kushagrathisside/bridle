// The product walkthrough: tabs that move a highlight over one screenshot, or switch screenshots. It follows the tabs pattern:
// arrow keys move between tabs, Home and End jump, and only the selected tab is in the tab order.
export {};

const root = document.querySelector<HTMLElement>('[data-tour]');

if (root) {
  const tabs = [...root.querySelectorAll<HTMLButtonElement>('[role="tab"]')];
  const stage = root.querySelector<HTMLElement>('#tour-stage');
  const frame = root.querySelector<HTMLElement>('[data-frame]');
  const images = [...root.querySelectorAll<HTMLImageElement>('[data-shot]')];
  const caption = root.querySelector<HTMLElement>('[data-caption]');
  const more = root.querySelector<HTMLAnchorElement>('[data-more]');

  const numbers = (text: string | undefined, fallback: number[]): number[] => {
    const parsed = (text ?? '').split(',').map(Number);
    return parsed.length === 4 && parsed.every(Number.isFinite) ? parsed : fallback;
  };

  function select(tab: HTMLButtonElement, focus: boolean): void {
    if (!frame) return;
    for (const other of tabs) {
      const on = other === tab;
      other.setAttribute('aria-selected', String(on));
      other.tabIndex = on ? 0 : -1;
    }
    const id = tab.dataset.id;
    for (const image of images) image.hidden = image.dataset.shot !== id;
    const [x, y, w, h] = numbers(tab.dataset.region, [0, 0, 1, 1]);
    const [cx, cy, cw, ch] = numbers(tab.dataset.crop, [x, y, w, h]);
    const style = frame.style;
    style.setProperty('--x', String(x));
    style.setProperty('--y', String(y));
    style.setProperty('--w', String(w));
    style.setProperty('--h', String(h));
    style.setProperty('--cx', String(cx));
    style.setProperty('--cy', String(cy));
    style.setProperty('--cw', String(cw));
    style.setProperty('--ch', String(ch));
    stage?.setAttribute('aria-labelledby', tab.id);
    if (caption) caption.textContent = tab.querySelector('.tour__text')?.textContent ?? '';
    if (more) {
      more.textContent = tab.dataset.moreLabel ?? '';
      more.hidden = !tab.dataset.moreHref;
      if (tab.dataset.moreHref) more.href = tab.dataset.moreHref;
    }
    if (focus) tab.focus();
  }

  tabs.forEach((tab) => tab.addEventListener('click', () => select(tab, false)));
  root.querySelector('[role="tablist"]')?.addEventListener('keydown', (event) => {
    const key = (event as KeyboardEvent).key;
    const at = tabs.findIndex((tab) => tab.getAttribute('aria-selected') === 'true');
    const next = key === 'ArrowDown' || key === 'ArrowRight' ? at + 1 : key === 'ArrowUp' || key === 'ArrowLeft' ? at - 1 : key === 'Home' ? 0 : key === 'End' ? tabs.length - 1 : -1;
    if (next < 0) return;
    event.preventDefault();
    select(tabs[(next + tabs.length) % tabs.length], true);
  });

  select(tabs[0], false);
}
