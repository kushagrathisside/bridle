// The policy lab. Every result comes from the recorded answers of the real policy. The first one is in the page; the others wait in
// <template> elements and are copied in the first time they are chosen. The script chooses which one is visible and reveals its six
// stages one after the other. Without it, the first result is shown with all its stages.
export {};

const lab = document.querySelector<HTMLElement>('[data-lab]');

if (lab) {
  const buttons = [...lab.querySelectorAll<HTMLButtonElement>('[data-run]')];
  const stage = lab.querySelector<HTMLElement>('.lab__stage');
  const status = document.querySelector<HTMLElement>('[data-lab-status]');
  const reduced = (): boolean => matchMedia('(prefers-reduced-motion: reduce)').matches;
  let timers: number[] = [];

  lab.classList.add('is-ready');

  function clearTimers(): void {
    timers.forEach((timer) => window.clearTimeout(timer));
    timers = [];
  }

  function announce(panel: HTMLElement): void {
    if (!status) return;
    const verdict = panel.querySelector('.step--decide .stamp')?.textContent ?? panel.querySelector('.fail .stamp')?.textContent ?? '';
    const rule = panel.querySelector('.decision .mono')?.textContent ?? '';
    status.textContent = `${verdict}. ${rule}`.trim();
  }

  function play(panel: HTMLElement): void {
    clearTimers();
    const steps = [...panel.querySelectorAll<HTMLElement>('.step')];
    steps.forEach((step) => step.classList.remove('is-on'));
    if (!steps.length) return;
    if (reduced()) {
      steps.forEach((step) => step.classList.add('is-on'));
      announce(panel);
      return;
    }
    steps.forEach((step, index) => {
      timers.push(
        window.setTimeout(() => {
          step.classList.add('is-on');
          if (index === steps.length - 1) announce(panel);
        }, 120 + index * 380),
      );
    });
  }

  // Repeated edits: each press of the button is one more call, and the fifth is refused.
  function setUpLoop(panel: HTMLElement): void {
    const rows = [...panel.querySelectorAll<HTMLElement>('[data-loop] .loop__row')];
    const count = panel.querySelector<HTMLElement>('[data-loop-count]');
    const next = panel.querySelector<HTMLButtonElement>('[data-loop-next]');
    const reset = panel.querySelector<HTMLButtonElement>('[data-loop-reset]');
    let shown = 1;

    function showRows(): void {
      rows.forEach((row, index) => {
        row.hidden = index >= shown;
      });
      if (count) count.textContent = `call ${shown} of ${rows.length}`;
      if (next) next.disabled = shown >= rows.length;
      const last = rows[shown - 1];
      if (status && last) status.textContent = `Call ${shown}: ${last.querySelector('.stamp')?.textContent ?? ''}. ${last.querySelector('.loop__rule')?.textContent ?? ''}`;
    }

    next?.addEventListener('click', () => {
      shown = Math.min(rows.length, shown + 1);
      showRows();
    });
    reset?.addEventListener('click', () => {
      shown = 1;
      showRows();
    });
    showRows();
  }

  // The panel for `id`, copied in from its template the first time it is asked for.
  function panelFor(id: string): HTMLElement | null {
    let panel = lab!.querySelector<HTMLElement>(`[data-panel="${id}"]`);
    if (!panel) {
      const template = lab!.querySelector<HTMLTemplateElement>(`template[data-template="${id}"]`);
      if (!template || !stage) return null;
      stage.insertBefore(template.content.cloneNode(true), stage.querySelector('.lab__log'));
      panel = lab!.querySelector<HTMLElement>(`[data-panel="${id}"]`);
      if (panel && id === 'loop') setUpLoop(panel);
    }
    return panel;
  }

  function show(id: string): void {
    const panel = panelFor(id);
    if (!panel) return;
    for (const button of buttons) button.setAttribute('aria-pressed', String(button.dataset.run === id));
    lab!.querySelectorAll<HTMLElement>('[data-panel]').forEach((other) => {
      other.hidden = other !== panel;
    });
    if (id === 'loop') {
      const first = panel.querySelector<HTMLElement>('.loop__row');
      if (status && first) status.textContent = `Call 1: ${first.querySelector('.stamp')?.textContent ?? ''}.`;
    } else {
      play(panel);
    }
  }

  buttons.forEach((button) => button.addEventListener('click', () => show(button.dataset.run ?? '')));
  const current = buttons.find((button) => button.getAttribute('aria-pressed') === 'true');
  if (current) show(current.dataset.run ?? '');
}
