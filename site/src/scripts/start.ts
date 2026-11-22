// The install tabs: arrow keys move between them, and only the selected one is in the tab order.
export {};

const root = document.querySelector<HTMLElement>('[data-start]');

if (root) {
  const tabs = [...root.querySelectorAll<HTMLButtonElement>('[role="tab"]')];

  function select(tab: HTMLButtonElement, focus: boolean): void {
    for (const other of tabs) {
      const on = other === tab;
      other.setAttribute('aria-selected', String(on));
      other.tabIndex = on ? 0 : -1;
      const panel = document.getElementById(other.getAttribute('aria-controls') ?? '');
      if (panel) panel.hidden = !on;
    }
    if (focus) tab.focus();
  }

  tabs.forEach((tab) => tab.addEventListener('click', () => select(tab, false)));
  root.querySelector('[role="tablist"]')?.addEventListener('keydown', (event) => {
    const key = (event as KeyboardEvent).key;
    const at = tabs.findIndex((tab) => tab.getAttribute('aria-selected') === 'true');
    const next = key === 'ArrowRight' || key === 'ArrowDown' ? at + 1 : key === 'ArrowLeft' || key === 'ArrowUp' ? at - 1 : key === 'Home' ? 0 : key === 'End' ? tabs.length - 1 : -1;
    if (next < 0) return;
    event.preventDefault();
    select(tabs[(next + tabs.length) % tabs.length], true);
  });
}
