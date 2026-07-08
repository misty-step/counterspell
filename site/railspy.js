/* Contents-rail scroll spy for the docs-split get-started page. The window
   scrolls (the .msk-flow page opts out of the kit's inner-column scroll), so
   the active section is decided against the viewport. An IntersectionObserver
   fires the recompute; the section whose top has crossed a trigger line near
   the top of the viewport wins, and its rail link lights up. Rail links are
   plain in-page anchors, so navigation still works with JS disabled. */
(() => {
  const rail = document.querySelector('.msk-rail');
  const doc = document.querySelector('.msk-doc');
  if (!rail || !doc) return;

  const links = [...rail.querySelectorAll('a[href^="#"]')];
  const sections = links
    .map((a) => document.getElementById(a.getAttribute('href').slice(1)))
    .filter(Boolean);
  if (!sections.length) return;

  const linkFor = new Map(
    links.map((a) => [a.getAttribute('href').slice(1), a])
  );
  let current = null;

  function setActive(id) {
    if (id === current) return;
    current = id;
    for (const a of links) {
      const on = a.getAttribute('href') === '#' + id;
      a.classList.toggle('is-current', on);
      if (on) a.setAttribute('aria-current', 'true');
      else a.removeAttribute('aria-current');
    }
  }

  function recompute() {
    const vh = window.innerHeight;
    const probe = Math.min(vh * 0.3, 240);
    const maxScroll = document.documentElement.scrollHeight - vh;
    const y = window.scrollY;

    // base rule: the active section is the last one whose heading has risen
    // to or above the probe line near the top of the viewport
    let idx = 0;
    for (let i = 0; i < sections.length; i++) {
      if (sections[i].getBoundingClientRect().top <= probe) idx = i;
    }

    // short trailing sections can never lift their heading to the probe — the
    // page runs out of scroll first. Find the last section that CAN, then hand
    // the remaining ones their share of the final scroll stretch so each still
    // lights up on the way down (no dead whitespace, unlike padding the page).
    let lastReachable = 0;
    for (let i = 0; i < sections.length; i++) {
      if (sections[i].offsetTop - probe <= maxScroll) lastReachable = i;
    }
    if (idx >= lastReachable && lastReachable < sections.length - 1) {
      const yReach = sections[lastReachable].offsetTop - probe;
      if (y >= yReach && maxScroll > yReach) {
        const frac = (y - yReach) / (maxScroll - yReach); // 0..1 across the tail
        const extra = Math.round(frac * (sections.length - 1 - lastReachable));
        idx = Math.max(idx, lastReachable + extra);
      }
    }

    setActive(sections[Math.min(idx, sections.length - 1)].id);
  }

  const io = new IntersectionObserver(recompute, {
    threshold: [0, 0.25, 0.5, 1],
  });
  sections.forEach((s) => io.observe(s));
  window.addEventListener('scroll', recompute, { passive: true });
  window.addEventListener('resize', recompute);
  recompute();
})();
