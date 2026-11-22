// The recording in the hero. The page ships a still image first (it is what loads and what the layout reserves). Once the page
// has loaded, the video is fetched and played, unless the visitor asked for less motion or less data. A pause control is
// always there while it plays, and it is the only way the video runs for a visitor who asked for reduced motion.
export {};

const figure = document.querySelector<HTMLElement>('[data-demo]');
const video = figure?.querySelector<HTMLVideoElement>('[data-demo-video]');
const toggle = figure?.querySelector<HTMLButtonElement>('[data-demo-toggle]');
const frame = figure?.querySelector<HTMLElement>('.shot');

interface SaveData {
  saveData?: boolean;
}

function wantsLess(): boolean {
  const connection = (navigator as Navigator & { connection?: SaveData }).connection;
  return matchMedia('(prefers-reduced-motion: reduce)').matches || connection?.saveData === true;
}

function setPressed(paused: boolean): void {
  toggle?.setAttribute('aria-pressed', String(paused));
  if (toggle) toggle.dataset.paused = String(paused);
}

async function start(): Promise<void> {
  if (!video || !frame) return;
  video.src = video.dataset.src ?? '';
  try {
    await video.play();
    frame.dataset.playing = 'true';
    setPressed(false);
  } catch {
    // The browser refused to start it, or the file is missing. The still image stays, and nothing is broken.
    video.removeAttribute('src');
    video.load();
  }
}

if (video && frame && toggle) {
  toggle.hidden = false;
  video.addEventListener('error', () => {
    delete frame.dataset.playing;
    toggle.hidden = true;
  });
  toggle.addEventListener('click', () => {
    if (!video.getAttribute('src')) {
      void start();
      return;
    }
    if (video.paused) {
      void video.play();
      setPressed(false);
    } else {
      video.pause();
      setPressed(true);
    }
  });
  setPressed(true);
  if (!wantsLess()) {
    const begin = () => window.setTimeout(() => void start(), 400);
    if (document.readyState === 'complete') begin();
    else window.addEventListener('load', begin, { once: true });
  }
}
