// The architecture diagram: choose which stage is described, choose the outcome of the call, and trace a call through the stages.
// Everything it shows is already in the page as HTML; the script only changes which part is visible and highlighted.
export {};

const root = document.querySelector<HTMLElement>('[data-arch]');

if (root) {
  const stageButtons = [...root.querySelectorAll<HTMLButtonElement>('[data-select]')];
  const details = [...root.querySelectorAll<HTMLElement>('[data-detail]')];
  const outcomeButtons = [...root.querySelectorAll<HTMLButtonElement>('[data-outcome]')];
  const stages = [...root.querySelectorAll<HTMLElement>('[data-stage]')];
  const trace = root.querySelector<HTMLButtonElement>('[data-trace]');
  const note = root.querySelector<HTMLElement>('[data-trace-note]');

  // The stages a call passes, in order. A denied call is written to the record and goes back to the model; it never runs.
  const PATHS: Record<string, string[]> = {
    allow: ['model', 'host', 'policy', 'tool', 'result', 'log'],
    deny: ['model', 'host', 'policy', 'log'],
  };
  const SAYS: Record<string, Record<string, string>> = {
    model: { allow: 'The model asks for a tool call.', deny: 'The model asks for a tool call.' },
    host: { allow: 'The agent loop receives it and asks the policy first.', deny: 'The agent loop receives it and asks the policy first.' },
    policy: { allow: 'The policy classifies the call and answers allow.', deny: 'The policy answers ask or deny. Either way the call is blocked.' },
    tool: { allow: 'The allowed call runs, with your account’s rights.', deny: '' },
    result: { allow: 'The output is redacted before the model reads it.', deny: '' },
    log: { allow: 'The decision is written to the event log.', deny: 'The decision and its rule are written to the event log.' },
  };

  let timer = 0;
  let outcome = 'allow';

  function selectStage(id: string): void {
    for (const button of stageButtons) button.setAttribute('aria-pressed', String(button.dataset.select === id));
    for (const detail of details) detail.hidden = detail.dataset.detail !== id;
  }

  function setOutcome(next: string): void {
    outcome = next;
    root!.dataset.outcome = next;
    for (const button of outcomeButtons) button.setAttribute('aria-pressed', String(button.dataset.outcome === next));
    stopTrace();
  }

  function mark(active: string | null, reached: string[]): void {
    for (const stage of stages) {
      const id = stage.dataset.stage ?? '';
      stage.classList.toggle('is-active', id === active);
      stage.classList.toggle('is-reached', reached.includes(id));
    }
  }

  function stopTrace(): void {
    window.clearTimeout(timer);
    timer = 0;
    root!.classList.remove('is-tracing');
    mark(null, []);
    if (trace) trace.textContent = 'Trace a call';
    if (note) note.textContent = '';
  }

  function runTrace(): void {
    if (timer) return stopTrace();
    const path = PATHS[outcome];
    const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
    root!.classList.add('is-tracing');
    if (trace) trace.textContent = 'Stop';
    if (reduced) {
      // No animation: show the whole path at once, with the sentence for each stage in the note.
      mark(null, path);
      if (note) note.textContent = path.map((id) => SAYS[id][outcome]).filter(Boolean).join(' ');
      return;
    }
    let at = 0;
    const step = (): void => {
      const id = path[at];
      mark(id, path.slice(0, at + 1));
      selectStage(id);
      if (note) note.textContent = SAYS[id][outcome];
      at += 1;
      if (at < path.length) {
        timer = window.setTimeout(step, 1100);
      } else {
        timer = window.setTimeout(() => {
          timer = 0;
          if (trace) trace.textContent = 'Trace a call';
          root!.classList.remove('is-tracing');
          mark(null, path);
        }, 1400);
      }
    };
    step();
  }

  stageButtons.forEach((button) => button.addEventListener('click', () => selectStage(button.dataset.select ?? 'policy')));
  outcomeButtons.forEach((button) => button.addEventListener('click', () => setOutcome(button.dataset.outcome ?? 'allow')));
  trace?.addEventListener('click', runTrace);
}
