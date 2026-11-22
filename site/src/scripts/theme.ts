// The theme toggle. The first paint is handled by the inline script in the layout; this keeps the buttons in step and stores
// an explicit choice under Starlight's key, so the documentation follows it.
export {};

const KEY = 'starlight-theme';
const root = document.documentElement;
const buttons = document.querySelectorAll<HTMLButtonElement>('[data-theme-toggle]');

type Theme = 'light' | 'dark';
const current = (): Theme => (root.dataset.theme === 'light' ? 'light' : 'dark');

function sync(): void {
  const next: Theme = current() === 'light' ? 'dark' : 'light';
  buttons.forEach((button) => {
    button.setAttribute('aria-label', `Switch to ${next} theme`);
  });
}

function stored(): string | null {
  try {
    return localStorage.getItem(KEY);
  } catch {
    return null;
  }
}

buttons.forEach((button) => {
  button.addEventListener('click', () => {
    const next: Theme = current() === 'light' ? 'dark' : 'light';
    root.dataset.theme = next;
    try {
      localStorage.setItem(KEY, next);
    } catch {
      /* storage can be blocked; the choice then lasts for this page only */
    }
    sync();
  });
});

// With no stored choice the page follows the system setting, also while it is open.
matchMedia('(prefers-color-scheme: light)').addEventListener('change', (event) => {
  if (!stored()) {
    root.dataset.theme = event.matches ? 'light' : 'dark';
    sync();
  }
});

sync();
