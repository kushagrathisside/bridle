// The navigation: the phone menu, the bar's border once the page scrolls, and the link of the section in view.
export {};

const header = document.querySelector<HTMLElement>('[data-nav]');
const toggle = document.querySelector<HTMLButtonElement>('[data-menu-toggle]');
const panel = document.querySelector<HTMLElement>('[data-menu]');

function setMenu(open: boolean): void {
  if (!toggle || !panel) return;
  panel.hidden = !open;
  toggle.setAttribute('aria-expanded', String(open));
  toggle.setAttribute('aria-label', open ? 'Close the menu' : 'Open the menu');
  document.documentElement.classList.toggle('menu-open', open);
}

toggle?.addEventListener('click', () => setMenu(toggle.getAttribute('aria-expanded') !== 'true'));
panel?.addEventListener('click', (event) => {
  if (event.target instanceof Element && event.target.closest('a')) setMenu(false);
});
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape' && toggle?.getAttribute('aria-expanded') === 'true') {
    setMenu(false);
    toggle.focus();
  }
});
matchMedia('(min-width: 56rem)').addEventListener('change', (event) => {
  if (event.matches) setMenu(false);
});

// A border under the bar once the page has moved.
let queued = false;
function mark(): void {
  queued = false;
  header?.classList.toggle('is-scrolled', window.scrollY > 8);
}
window.addEventListener(
  'scroll',
  () => {
    if (!queued) {
      queued = true;
      requestAnimationFrame(mark);
    }
  },
  { passive: true },
);
mark();

// The link of the section that is in view is marked, for people who use the bar to find their place.
const links = new Map<string, HTMLAnchorElement>();
document.querySelectorAll<HTMLAnchorElement>('.nav__links a[href*="#"]').forEach((link) => {
  const id = link.hash.slice(1);
  if (id) links.set(id, link);
});
if ('IntersectionObserver' in window && links.size) {
  const observer = new IntersectionObserver(
    (entries) => {
      for (const entry of entries) {
        const link = links.get(entry.target.id);
        if (!link) continue;
        if (entry.isIntersecting) {
          links.forEach((other) => other.removeAttribute('aria-current'));
          link.setAttribute('aria-current', 'true');
        } else if (link.getAttribute('aria-current') === 'true') {
          link.removeAttribute('aria-current');
        }
      }
    },
    { rootMargin: '-45% 0px -50% 0px' },
  );
  links.forEach((_, id) => {
    const section = document.getElementById(id);
    if (section) observer.observe(section);
  });
}
