// IOGP rule editor: auto-suggestion while typing.
// Tab accepts the top suggestion plus the grey ghost continuation; → at the end accepts the next
// ghost word; ↑/↓ + Enter pick a word from the list; Ctrl+Space opens the list; Esc closes it.
(function () {
  const ed = document.getElementById('ed');
  if (!ed) return;
  const box = document.getElementById('rule'), mirror = document.getElementById('ac-mirror');
  const list = document.getElementById('ac-list'), msg = document.getElementById('ac-msg');
  const menus = document.getElementById('ac-menus'), result = document.getElementById('ac-result');
  const status = document.getElementById('ac-status');
  let sug = null, sel = 0, open = false, seq = 0, timer = null, lastPanel = null, lastText = null, pendingTab = false;

  const esc = s => s.replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const atEnd = () => box.selectionStart === box.value.length && box.selectionEnd === box.value.length;

  function paintMirror() {
    const t = box.value;
    let html;
    if (sug && sug.error && sug.partial_start < t.length) {
      html = esc(t.slice(0, sug.partial_start)) + '<span class="bad">' + esc(t.slice(sug.partial_start, sug.bad_end)) + '</span>' + esc(t.slice(sug.bad_end));
    } else {
      html = esc(t);
      const g = ghostText();
      if (g && atEnd() && document.activeElement === box) html += '<span class="gh">' + esc(g) + '</span>';
    }
    mirror.innerHTML = html + '​';
    mirror.scrollTop = box.scrollTop;
  }

  // what Tab would append: rest of the top option + the ghost continuation
  function ghostText() {
    if (!sug || sug.error || !sug.options.length) return '';
    const best = sug.options[0].w, t = box.value, p = sug.partial;
    let s;
    if (p && best.toLowerCase().startsWith(p.toLowerCase())) s = best.slice(p.length);
    else if (!p && sug.confident) s = (needsSpace(t, best) ? ' ' : '') + best;
    else return '';
    s += sug.ghost || '';
    if (s && !/[ .]$/.test(s)) s += ' ';
    return s;
  }
  const needsSpace = (t, w) => t.length > 0 && !/\s$/.test(t) && w !== '.';

  function renderList() {
    if (!sug || !open || !sug.options.length) { list.innerHTML = ''; list.style.display = 'none'; msg.style.display = ''; return; }
    // while the list is open, the problem is shown at its top instead of under it
    msg.style.display = sug.error ? 'none' : '';
    const head = sug.error ? '<li class="head">' + esc(sug.error) + '</li>' : '';
    const p = sug.partial.toLowerCase();
    list.innerHTML = head + sug.options.slice(0, 12).map((o, i) => {
      const w = esc(o.w);
      const label = (!o.fix && p && o.w.toLowerCase().startsWith(p)) ? '<b>' + esc(o.w.slice(0, p.length)) + '</b>' + esc(o.w.slice(p.length)) : w;
      return '<li role="option" data-i="' + i + '" class="' + (i === sel ? 'sel' : '') + '">' + label +
        (o.fix ? '<span class="fx">fix “' + esc(o.fix) + '”</span>' : '') + '<span class="g">' + esc(o.g) + '</span></li>';
    }).join('') + (sug.options.length > 12 ? '<li class="more">… ' + (sug.options.length - 12) + ' more — keep typing to narrow</li>' : '');
    list.style.display = 'block';
  }

  function renderMenus() {
    if (!sug) return;
    menus.innerHTML = sug.groups.map(([g, ws], gi) =>
      '<div class="menu"><h4 style="background:hsl(' + ((200 + gi * 33) % 360) + ',40%,40%)">' + esc(g) + '</h4><div class="items">' +
      ws.slice(0, 200).map(w => '<button type="button" data-w="' + esc(w) + '">' + esc(w) + '</button>').join('') + '</div></div>').join('');
  }

  function insert(word, withGhost) {
    if (sug.error && sug.bad_end < box.value.trimEnd().length) {
      // a spelling fix replaces just the unreadable word; the rest of the sentence stays
      const v = box.value, pos = sug.partial_start + word.length;
      box.value = v.slice(0, sug.partial_start) + word + v.slice(sug.bad_end);
      box.focus(); box.setSelectionRange(pos, pos); update(0); return;
    }
    let t = box.value.slice(0, sug.partial_start);
    if (word === '.') t = t.replace(/\s+$/, '');
    else if (t.length && !/\s$/.test(t)) t += ' ';
    t += word;
    if (withGhost && sug.ghost) t += sug.ghost;
    if (!/\.$/.test(t)) t += ' ';
    box.value = t;
    box.focus();
    box.setSelectionRange(t.length, t.length);
    update(0);
  }

  function update(delay) {
    clearTimeout(timer);
    timer = setTimeout(async () => {
      const my = ++seq, text = box.value;
      let r;
      try { r = await fetch('/suggest', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text }) }); }
      catch (e) { status.textContent = 'offline'; return; }
      if (my !== seq || !r.ok) return;
      sug = await r.json();
      lastText = text;
      sel = 0;
      msg.textContent = sug.error || '';
      status.innerHTML = sug.complete ? '✓ complete sentence' :
        (text.trim() ? '<kbd>Tab</kbd> accept · <kbd>↑</kbd><kbd>↓</kbd> <kbd>Enter</kbd> pick · <kbd>Ctrl</kbd>+<kbd>Space</kbd> all words' : '');
      if (sug.error) open = true;
      paintMirror(); renderList(); renderMenus();
      if (sug.complete) panel(text); else if (lastPanel !== null) { result.innerHTML = ''; lastPanel = null; }
      if (pendingTab && text === box.value && sug.options.length) { pendingTab = false; insert(sug.options[0].w, !sug.error && (sug.confident || sug.partial !== '')); }
      pendingTab = false;
    }, delay === undefined ? 90 : delay);
  }

  async function panel(text) {
    if (text === lastPanel) return;
    lastPanel = text;
    const body = new URLSearchParams({ text, id: ed.dataset.id || '' });
    const r = await fetch('/panel', { method: 'POST', body });
    if (r.headers.get('HX-Redirect')) { location.href = r.headers.get('HX-Redirect'); return; }
    if (box.value !== text) return;
    result.innerHTML = await r.text();
    if (window.htmx) htmx.process(result);
  }

  box.addEventListener('input', () => { open = true; paintMirror(); update(); });
  box.addEventListener('scroll', () => { mirror.scrollTop = box.scrollTop; });
  box.addEventListener('click', () => paintMirror());
  box.addEventListener('blur', () => setTimeout(() => { open = false; renderList(); paintMirror(); }, 150));
  box.addEventListener('focus', () => paintMirror());
  box.addEventListener('keydown', e => {
    const n = sug ? Math.min(sug.options.length, 12) : 0;
    if (e.key === ' ' && e.ctrlKey) { e.preventDefault(); open = true; renderList(); return; }
    if (e.key === 'Escape') { open = false; renderList(); return; }
    if (e.key === 'Tab' && !e.shiftKey) {
      // Tab never leaves the box while there is something to accept (or a request in flight)
      if (!box.value.trim() && !(sug && n)) return;
      e.preventDefault();
      if (!sug || box.value !== lastText) { pendingTab = true; update(0); return; }
      if (!n) return;
      if (open && sel > 0) insert(sug.options[sel].w, false);
      else insert(sug.options[0].w, !sug.error && (sug.confident || sug.partial !== ''));
      return;
    }
    if (open && n && (e.key === 'ArrowDown' || e.key === 'ArrowUp')) {
      e.preventDefault(); sel = (sel + (e.key === 'ArrowDown' ? 1 : n - 1)) % n; renderList(); return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      if (open && n) insert(sug.options[sel].w, false);
      return;
    }
    if (e.key === 'ArrowRight' && atEnd() && sug && !sug.error) {
      const g = ghostText().replace(/^\s+/, '');
      if (g) { e.preventDefault(); const w = g.split(/\s+/)[0]; const m = sug.options.find(o => o.w === w || o.w.toLowerCase().startsWith(w.toLowerCase()));
        insert(m ? m.w : sug.options[0].w, false); }
    }
  });
  list.addEventListener('mousedown', e => { const li = e.target.closest('li[data-i]'); if (li) { e.preventDefault(); insert(sug.options[+li.dataset.i].w, false); } });
  menus.addEventListener('click', e => { const b = e.target.closest('button[data-w]'); if (b) insert(b.dataset.w, false); });
  document.getElementById('ac-clear').addEventListener('click', () => { box.value = ''; box.focus(); update(0); });
  document.querySelectorAll('.tplbtn').forEach(b => b.addEventListener('click', () => {
    box.value = b.dataset.text; box.focus(); box.setSelectionRange(box.value.length, box.value.length); open = false; update(0);
  }));
  update(0);
})();
