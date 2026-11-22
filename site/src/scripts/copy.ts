// Copy buttons. A button names its text in `data-copy-text`, or the element that holds it in `data-copy-target`. The result is
// shown on the button and announced in the nearest live region, so the control is never silent.
export {};

async function write(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // The clipboard API needs a secure page and a permission. Fall back to selecting a hidden field.
    const field = document.createElement('textarea');
    field.value = text;
    field.setAttribute('readonly', '');
    field.style.position = 'fixed';
    field.style.opacity = '0';
    document.body.append(field);
    field.select();
    let ok = false;
    try {
      ok = document.execCommand('copy');
    } catch {
      ok = false;
    }
    field.remove();
    return ok;
  }
}

document.addEventListener('click', async (event) => {
  const button = event.target instanceof Element ? event.target.closest<HTMLButtonElement>('[data-copy-text], [data-copy-target]') : null;
  if (!button) return;
  const target = button.dataset.copyTarget ? document.getElementById(button.dataset.copyTarget) : null;
  const text = button.dataset.copyText ?? target?.innerText.replace(/ /g, ' ').trimEnd() ?? '';
  if (!text) return;
  const ok = await write(text);
  button.dataset.state = ok ? 'copied' : 'failed';
  const status = button.parentElement?.querySelector<HTMLElement>('[data-copy-status]');
  if (status) status.textContent = ok ? 'Copied to the clipboard' : 'Could not copy. Select the text and copy it yourself.';
  window.setTimeout(() => {
    delete button.dataset.state;
    if (status) status.textContent = '';
  }, 1800);
});
