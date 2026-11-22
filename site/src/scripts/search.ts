// Search on the home page: a dialog that queries the Pagefind index Starlight builds for the documentation. The library is
// loaded the first time the dialog opens, so a visitor who never searches never downloads it.
export {};

interface PagefindResult {
  url: string;
  excerpt: string;
  meta: { title?: string };
}
interface PagefindApi {
  options(options: { baseUrl: string }): Promise<void>;
  init(): Promise<void>;
  search(query: string): Promise<{ results: { data(): Promise<PagefindResult> }[] }>;
}

const dialog = document.querySelector<HTMLDialogElement>('[data-search]');
const input = dialog?.querySelector<HTMLInputElement>('input');
const list = dialog?.querySelector<HTMLUListElement>('[data-search-results]');
const status = dialog?.querySelector<HTMLElement>('[data-search-status]');
const base = dialog?.dataset.base ?? '';

let engine: Promise<PagefindApi> | null = null;
let opener: HTMLElement | null = null;
let timer = 0;
let latest = 0;

function say(text: string): void {
  if (status) status.textContent = text;
}

function load(): Promise<PagefindApi> {
  engine ??= (async () => {
    const url = `${base}/pagefind/pagefind.js`;
    const api = (await import(/* @vite-ignore */ url)) as PagefindApi;
    await api.options({ baseUrl: `${base}/` });
    await api.init();
    return api;
  })();
  // A failed load is tried again the next time the dialog opens.
  engine.catch(() => {
    engine = null;
  });
  return engine;
}

// An excerpt is text with <mark> around the matches. It is rebuilt from text nodes, so no markup from the index is inserted.
function excerptOf(html: string): DocumentFragment {
  const fragment = document.createDocumentFragment();
  const parsed = new DOMParser().parseFromString(html, 'text/html');
  parsed.body.childNodes.forEach((node) => {
    if (node instanceof HTMLElement && node.tagName === 'MARK') {
      const mark = document.createElement('mark');
      mark.textContent = node.textContent;
      fragment.append(mark);
    } else {
      fragment.append(document.createTextNode(node.textContent ?? ''));
    }
  });
  return fragment;
}

function render(results: PagefindResult[]): void {
  if (!list || !input) return;
  list.replaceChildren(
    ...results.map((result, index) => {
      const item = document.createElement('li');
      item.setAttribute('role', 'option');
      item.id = `search-option-${index}`;
      const link = document.createElement('a');
      link.href = result.url;
      const title = document.createElement('strong');
      title.textContent = result.meta.title ?? result.url;
      const text = document.createElement('span');
      text.append(excerptOf(result.excerpt));
      link.append(title, text);
      item.append(link);
      return item;
    }),
  );
  input.setAttribute('aria-expanded', String(results.length > 0));
  select(results.length ? 0 : -1);
}

function select(index: number): void {
  if (!list || !input) return;
  const items = [...list.children] as HTMLElement[];
  items.forEach((item, at) => item.setAttribute('aria-selected', String(at === index)));
  if (index >= 0 && items[index]) {
    input.setAttribute('aria-activedescendant', items[index].id);
    items[index].scrollIntoView({ block: 'nearest' });
  } else {
    input.removeAttribute('aria-activedescendant');
  }
}

function selected(): number {
  return list ? [...list.children].findIndex((item) => item.getAttribute('aria-selected') === 'true') : -1;
}

async function run(query: string): Promise<void> {
  const ticket = ++latest;
  if (!query.trim()) {
    list?.replaceChildren();
    input?.setAttribute('aria-expanded', 'false');
    say('Type to search the documentation.');
    return;
  }
  say('Searching…');
  try {
    const api = await load();
    const found = await api.search(query);
    const data = await Promise.all(found.results.slice(0, 8).map((result) => result.data()));
    if (ticket !== latest) return;
    render(data);
    say(data.length ? `${data.length} result${data.length === 1 ? '' : 's'}` : `No pages match “${query}”.`);
  } catch {
    if (ticket !== latest) return;
    list?.replaceChildren();
    say('Search is not available here. The index is built with the production site; run a full build to get it.');
  }
}

function open(): void {
  if (!dialog || !input) return;
  opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  if (!dialog.open) dialog.showModal();
  input.focus();
  input.select();
  say(input.value ? '' : 'Type to search the documentation.');
  void load().catch(() => undefined);
}

document.querySelectorAll('[data-search-open]').forEach((button) => button.addEventListener('click', open));

document.addEventListener('keydown', (event) => {
  const typing = event.target instanceof HTMLElement && /^(INPUT|TEXTAREA|SELECT)$/.test(event.target.tagName);
  if ((event.key === 'k' && (event.ctrlKey || event.metaKey)) || (event.key === '/' && !typing && !event.ctrlKey && !event.metaKey)) {
    event.preventDefault();
    open();
  }
});

input?.addEventListener('input', () => {
  window.clearTimeout(timer);
  timer = window.setTimeout(() => void run(input.value), 150);
});

input?.addEventListener('keydown', (event) => {
  const count = list?.children.length ?? 0;
  if (event.key === 'ArrowDown' && count) {
    event.preventDefault();
    select((selected() + 1) % count);
  } else if (event.key === 'ArrowUp' && count) {
    event.preventDefault();
    select((selected() - 1 + count) % count);
  } else if (event.key === 'Enter') {
    const link = list?.children[selected()]?.querySelector('a');
    if (link) {
      event.preventDefault();
      link.click();
    }
  }
});

// A click on the backdrop closes the dialog; a click inside it does not.
dialog?.addEventListener('click', (event) => {
  if (event.target === dialog) dialog.close();
});
dialog?.addEventListener('close', () => {
  opener?.focus();
  opener = null;
});
