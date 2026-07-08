/* Copy buttons for every code block. No deps: wraps each <pre>, injects a
   button with two inline Lucide glyphs (copy → check), and crossfades them
   on a successful clipboard write. */
(() => {
  const SVG = 'http://www.w3.org/2000/svg';

  function icon(kind) {
    const svg = document.createElementNS(SVG, 'svg');
    svg.setAttribute('class', `ae-icon msk-copy-${kind}`);
    svg.setAttribute('viewBox', '0 0 24 24');
    svg.setAttribute('fill', 'none');
    svg.setAttribute('stroke', 'currentColor');
    svg.setAttribute('stroke-width', '2');
    svg.setAttribute('stroke-linecap', 'round');
    svg.setAttribute('stroke-linejoin', 'round');
    svg.setAttribute('aria-hidden', 'true');
    const paths =
      kind === 'copy'
        ? [
            { tag: 'rect', attrs: { width: '14', height: '14', x: '8', y: '8', rx: '2', ry: '2' } },
            { tag: 'path', attrs: { d: 'M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2' } },
          ]
        : [{ tag: 'path', attrs: { d: 'M20 6 9 17l-5-5' } }];
    for (const p of paths) {
      const el = document.createElementNS(SVG, p.tag);
      for (const [k, v] of Object.entries(p.attrs)) el.setAttribute(k, v);
      svg.appendChild(el);
    }
    return svg;
  }

  const blocks = document.querySelectorAll('.ae-doc pre, main pre');
  blocks.forEach((pre) => {
    if (pre.closest('.msk-codewrap')) return;

    const wrap = document.createElement('div');
    wrap.className = 'msk-codewrap';
    pre.parentNode.insertBefore(wrap, pre);
    wrap.appendChild(pre);

    const btn = document.createElement('button');
    btn.type = 'button';
    btn.className = 'msk-copy';
    btn.setAttribute('aria-label', 'Copy code');
    btn.appendChild(icon('copy'));
    btn.appendChild(icon('check'));
    wrap.appendChild(btn);

    let timer = null;
    btn.addEventListener('click', async () => {
      const text = pre.innerText;
      try {
        if (navigator.clipboard && navigator.clipboard.writeText) {
          await navigator.clipboard.writeText(text);
        } else {
          const ta = document.createElement('textarea');
          ta.value = text;
          ta.style.position = 'fixed';
          ta.style.opacity = '0';
          document.body.appendChild(ta);
          ta.select();
          document.execCommand('copy');
          ta.remove();
        }
        btn.classList.add('is-copied');
        btn.setAttribute('aria-label', 'Copied');
        if (timer) clearTimeout(timer);
        timer = setTimeout(() => {
          btn.classList.remove('is-copied');
          btn.setAttribute('aria-label', 'Copy code');
        }, 1500);
      } catch (e) {
        btn.setAttribute('aria-label', 'Copy failed');
      }
    });
  });
})();
