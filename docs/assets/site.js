(function () {
  var calm = window.matchMedia('(prefers-reduced-motion: reduce)').matches;

  function all(selector, root) {
    return Array.prototype.slice.call((root || document).querySelectorAll(selector));
  }

  // Runs `start` while the element is on screen and `stop` when it leaves.
  function whileVisible(el, start, stop) {
    new IntersectionObserver(function (entries) {
      if (entries[0].isIntersecting) start();
      else if (stop) stop();
    }, { rootMargin: '80px 0px' }).observe(el);
  }

  // Calls `step` every `ms` while the element is visible; once, still, with reduced motion.
  function cycle(el, ms, step) {
    var timer = 0;
    step(0);
    if (calm) return;
    var n = 0;
    whileVisible(el, function () {
      clearInterval(timer);
      timer = setInterval(function () { step(++n); }, ms);
    }, function () {
      clearInterval(timer);
    });
  }

  /* ---------- Release info ---------- */

  // The appcast is the single source of truth for what the current release is, so
  // the page reads it instead of carrying a version number that goes stale.
  fetch('appcast.xml', { cache: 'no-cache' })
    .then(function (response) {
      if (!response.ok) throw new Error(response.status);
      return response.text();
    })
    .then(function (text) {
      var feed = new DOMParser().parseFromString(text, 'application/xml');
      var item = feed.querySelector('item');
      var enclosure = item && item.querySelector('enclosure');
      if (!enclosure) throw new Error('no enclosure');

      // generate_appcast writes shortVersionString as a child element of the
      // item, and falls back to titling the item with the version itself.
      var sparkleNS = 'http://www.andymatuschak.org/xml-namespaces/sparkle';
      var shortVersion = item.getElementsByTagNameNS(sparkleNS, 'shortVersionString')[0];
      var title = item.querySelector('title');
      var version = (shortVersion && shortVersion.textContent.trim()) ||
        (title && title.textContent.trim().replace(/^Vibra\s*/, '')) || '';

      var url = enclosure.getAttribute('url');
      var length = parseInt(enclosure.getAttribute('length'), 10);
      var size = length ? ' · ' + (length / 1048576).toFixed(1) + ' MB' : '';

      all('[data-download]').forEach(function (link) { link.href = url; });
      all('[data-download-label]').forEach(function (label) {
        label.textContent = version ? 'Download Vibra ' + version : 'Download Vibra';
      });
      if (version) all('[data-version]').forEach(function (el) { el.textContent = 'v' + version; });
      all('[data-meta]').forEach(function (el) {
        el.textContent = 'Universal · macOS 14+ · Signed & notarized' + size;
      });
    })
    .catch(function () {
      // Leave the links pointing at the releases page: still correct, just less direct.
    });

  /* ---------- Header ---------- */

  var topbar = document.querySelector('[data-topbar]');
  var topbarButton = topbar.querySelector('.button');
  var heroButton = document.getElementById('download');
  var stage = document.querySelector('[data-mock-stage]');
  var tiltTarget = stage.querySelector('.mock-viewport');
  var scrollQueued = false;

  function onScroll() {
    scrollQueued = false;
    topbar.classList.toggle('scrolled', window.scrollY > 24);
    // The header offers the download once the hero's own button has scrolled away.
    var past = heroButton.getBoundingClientRect().bottom < 64;
    topbar.classList.toggle('past-hero', past);
    topbarButton.tabIndex = past ? 0 : -1;
    tilt();
  }
  window.addEventListener('scroll', function () {
    if (!scrollQueued) {
      scrollQueued = true;
      requestAnimationFrame(onScroll);
    }
  }, { passive: true });

  var navLinks = all('.nav a');
  var sections = navLinks.map(function (link) { return document.querySelector(link.getAttribute('href')); });
  var sectionObserver = new IntersectionObserver(function (entries) {
    entries.forEach(function (entry) {
      var link = navLinks[sections.indexOf(entry.target)];
      if (entry.isIntersecting) link.setAttribute('aria-current', 'true');
      else link.removeAttribute('aria-current');
    });
  }, { rootMargin: '-45% 0px -50% 0px' });
  sections.forEach(function (section) { sectionObserver.observe(section); });

  /* ---------- Reveals ---------- */

  var revealer = new IntersectionObserver(function (entries) {
    entries.forEach(function (entry) {
      if (!entry.isIntersecting) return;
      entry.target.classList.add('in');
      revealer.unobserve(entry.target);
    });
  }, { rootMargin: '0px 0px -8% 0px', threshold: 0.12 });
  all('[data-reveal]').forEach(function (el) {
    if (calm) el.classList.add('in');
    else revealer.observe(el);
  });

  // Rings fill and other one-time entrances start the first time their block is seen.
  all('[data-mock], .card').forEach(function (el) {
    whileVisible(el, function () { el.classList.add('on'); });
  });

  all('[data-card]').forEach(function (card) {
    card.addEventListener('pointermove', function (e) {
      var r = card.getBoundingClientRect();
      card.style.setProperty('--mx', (e.clientX - r.left) + 'px');
      card.style.setProperty('--my', (e.clientY - r.top) + 'px');
    });
  });

  function pulse(x, y, amp) {
    document.dispatchEvent(new CustomEvent('vibra:pulse', { detail: { x: x, y: y, amp: amp } }));
  }

  all('[data-download]').forEach(function (button) {
    var last = 0;
    button.addEventListener('pointerenter', function () {
      if (performance.now() - last < 1500) return;
      last = performance.now();
      var r = button.getBoundingClientRect();
      pulse(r.left + r.width / 2, r.top + r.height / 2, 0.45);
    });
  });

  /* ---------- ASCII fields ---------- */

  var FACE_X = 0.49, FACE_Y = 0.31;      // the face in hero.jpg, as fractions of its size
  // The mark's bars as [x, width] pairs per row, in the icon's 512-unit artboard.
  var BARS = [
    [[84, 64], [364, 64]], [[97, 79], [336, 79]], [[116, 81], [315, 81]], [[134, 87], [291, 87]],
    [[152, 88], [272, 88]], [[173, 79], [260, 79]], [[198, 114]], [[226, 59]]
  ];

  function still() {
    document.documentElement.classList.add('still');
  }

  var heroCanvas = document.getElementById('hero-field');
  var ctaCanvas = document.getElementById('cta-field');
  if (!window.VibraField || !heroCanvas.getContext || !window.ResizeObserver) {
    still();
  } else {
    var photo = new Image();
    photo.src = 'hero.jpg';
    var font = document.fonts
      ? Promise.race([document.fonts.load('500 12px "JetBrains Mono"'), new Promise(function (done) { setTimeout(done, 1500); })])
      : Promise.resolve();
    font.then(function () {
      VibraField(ctaCanvas, { paint: paintMark, lo: 0.1, hi: 1.05, schools: 2600 });
    });
    Promise.all([photo.decode(), font]).then(function () {
      if (!VibraField(heroCanvas, { paint: paintPhoto, lo: 0.12, hi: 0.8, sharpen: 1.2, schools: 1500 })) still();
    }, still);
  }

  function paintPhoto(g, W, H, cell, k) {
    var iw = photo.naturalWidth, ih = photo.naturalHeight;
    // Wide screens get the photo full-bleed; tall ones shrink it so the face
    // and hands stay in frame above the copy.
    var tall = W / H < 0.9;
    var s = tall ? Math.max(W / iw, H * 0.62 / ih) : Math.max(W / iw, H / ih) * 1.04;
    var w = iw * s, h = ih * s;
    var x = Math.min(0, Math.max(W - w, W / 2 - FACE_X * w));
    var y = tall ? 48 : Math.min(0, Math.max(H - h, H * 0.27 - FACE_Y * h));
    g.drawImage(VibraField.shrink(photo, w * k), x, y, w, h);
    if (tall) {
      // Let the bottom of the photo dissolve instead of ending on a hard edge.
      var fade = g.createLinearGradient(0, y + h * 0.7, 0, y + h);
      fade.addColorStop(0, '#000');
      fade.addColorStop(1, 'rgba(0, 0, 0, 0)');
      g.globalCompositeOperation = 'destination-in';
      g.fillStyle = fade;
      g.fillRect(-W, -H, W * 3, H * 3);
      g.globalCompositeOperation = 'source-over';
    }
    return { x: x + FACE_X * w, y: y + FACE_Y * h };
  }

  // The mark is drawn straight onto the grid: every bar spans whole rows, so
  // each one reads as a clean line of tiles.
  function paintMark(g, W, H, cell) {
    var wide = W >= 820;
    var room = wide ? W * 0.44 : W * 0.86;
    var pitch = 4, unit;
    for (; pitch > 2; pitch--) {
      unit = pitch * cell / 33; // a bar and its gap are about 33 units tall
      if (344 * unit <= room && BARS.length * pitch * cell <= H * (wide ? 0.72 : 0.36)) break;
    }
    unit = pitch * cell / 33;
    var thick = Math.ceil(pitch / 2), inset = Math.floor((pitch - thick) / 2);
    var height = BARS.length * pitch * cell;
    var cx = wide ? W * 0.72 : W / 2;
    var top = Math.round((wide ? (H - height) / 2 : H * 0.07) / cell) * cell;
    var left = cx - (84 + 344 / 2) * unit;

    var fill = g.createLinearGradient(0, top, 0, top + height);
    fill.addColorStop(0, '#f6c08a');
    fill.addColorStop(0.5, '#efece4');
    fill.addColorStop(1, '#9cc4ee');
    g.fillStyle = fill;
    BARS.forEach(function (row, i) {
      var y = top + (i * pitch + inset) * cell;
      row.forEach(function (bar) {
        var x = left + bar[0] * unit, w = bar[1] * unit, h = thick * cell;
        g.beginPath();
        if (g.roundRect) g.roundRect(x, y, w, h, h / 2);
        else g.rect(x, y, w, h);
        g.fill();
      });
    });
    return { x: cx, y: top + height / 2 };
  }

  /* ---------- Product mockup ---------- */

  new ResizeObserver(function () {
    stage.style.setProperty('--mock-scale', Math.min(1, stage.clientWidth / 1200));
    tilt();
  }).observe(stage);

  // The window leans back while it peeks under the hero and stands up as it scrolls in.
  function tilt() {
    if (calm) return;
    var r = stage.getBoundingClientRect(), vh = window.innerHeight;
    var p = Math.min(1, Math.max(0, (vh - r.top) / (vh * 0.75)));
    tiltTarget.style.setProperty('--tilt', ((1 - p) * 16).toFixed(2) + 'deg');
    tiltTarget.style.setProperty('--lift', ((1 - p) * 30).toFixed(1) + 'px');
  }
  onScroll();

  var mock = document.querySelector('[data-mock]');
  var LABELS = { working: 'Working', waiting: 'Waiting', permission: 'Permission', idle: 'Idle' };
  var URGENCY = ['idle', 'working', 'waiting', 'permission'];
  var STATE_COLOR = {
    idle: 'var(--m-subtle)',
    working: 'var(--m-accent)',
    waiting: 'var(--m-warning)',
    permission: 'var(--m-danger)'
  };
  var BASE_FILES = [
    { name: 'token.rs', dir: 'src/auth', add: 29, del: 9, letter: 'M' },
    { name: 'auth_clock.rs', dir: 'tests', add: 61, del: 0, letter: 'A' }
  ];
  var agents, files;

  function setState(card, state) {
    card.dataset.state = state;
    card.querySelector('.m-state').textContent = LABELS[state];
  }

  function setAgent(name, state) {
    agents[name] = state;
    setState(mock.querySelector('[data-agent="' + name + '"]'), state);
    var counts = { working: 0, waiting: 0, permission: 0 };
    Object.keys(agents).forEach(function (key) {
      if (counts[agents[key]] !== undefined) counts[agents[key]]++;
    });
    var groups = [
      [counts.working, 'var(--m-accent)', 'working'],
      [counts.waiting, 'var(--m-warning)', 'waiting'],
      [counts.permission, 'var(--m-danger)', 'need permission']
    ].filter(function (group) { return group[0] > 0; });
    mock.querySelector('[data-agent-summary]').innerHTML = groups.length
      ? groups.map(function (group) {
        return '<i style="background:' + group[1] + '"></i>' + group[0] + ' ' + group[2];
      }).join('<span class="m-sep">·</span>')
      : '<i style="background:var(--m-subtle)"></i>No active agents';
    // The tab shows the most urgent state among its panes.
    var tabState = [agents.claude, agents.codex].sort(function (a, b) {
      return URGENCY.indexOf(b) - URGENCY.indexOf(a);
    })[0];
    mock.querySelector('[data-tab-dot]').style.setProperty('--dot', STATE_COLOR[tabState]);
  }

  function renderFiles(fresh) {
    var add = 0, del = 0;
    mock.querySelector('[data-files]').innerHTML = files.map(function (file) {
      add += file.add;
      del += file.del;
      return '<div class="m-file' + (file.name === fresh ? ' is-new' : '') + '"><span class="m-ext">RS</span>' +
        file.name + '<span class="m-dir">' + file.dir + '</span><span class="m-stat"><span class="m-add">+' + file.add +
        '</span>' + (file.del ? '<span class="m-del">−' + file.del + '</span>' : '') + '</span><span class="m-letter' +
        (file.letter === 'A' ? ' a' : '') + '">' + file.letter + '</span></div>';
    }).join('');
    all('[data-sum-add]', mock).forEach(function (el) { el.textContent = '+' + add; });
    all('[data-sum-del]', mock).forEach(function (el) { el.textContent = '−' + del; });
    mock.querySelector('[data-file-count]').textContent = files.length + ' files';
    mock.querySelector('[data-file-total]').textContent = files.length;
  }

  function addFile(file) {
    files.splice(1, 0, file);
    renderFiles(file.name);
  }

  // A pane's terminal: lines are appended and the oldest scroll off the top.
  function Term(name) {
    this.el = mock.querySelector('[data-term="' + name + '"]');
    this.tail = null;
  }
  Term.prototype.line = function (html, className) {
    var line = document.createElement('span');
    line.className = 'line' + (calm ? '' : ' enter') + (className ? ' ' + className : '');
    line.innerHTML = html;
    this.el.insertBefore(line, this.tail);
    this.el.scrollTop = this.el.scrollHeight;
    return line;
  };
  Term.prototype.clear = function () {
    this.el.textContent = '';
    this.tail = null;
  };

  var claude = new Term('claude'), codex = new Term('codex'), shell = new Term('shell');
  var LOOP = 15000;
  var queue = [], typing = [], elapsed = 0, spinner = null;

  function at(ms, run) {
    queue.push({ at: ms, run: run });
    queue.sort(function (a, b) { return a.at - b.at; });
  }

  // Types `text` into a fresh line after `prefix`, one character every `speed` ms.
  function type(term, prefix, text, speed) {
    var line = term.line(prefix + '<span></span>');
    typing.push({ el: line.lastChild, text: text, start: elapsed, speed: speed });
  }

  function recheck(start) {
    at(start, function () { shell.line('<span class="t-dim">[Running \'cargo check\']</span>'); });
    at(start + 600, function () { shell.line('    <span class="t-ok t-b">Checking</span> vibra'); });
    at(start + 1500, function () { shell.line('    <span class="t-ok t-b">Finished</span> `dev` profile in 2.41s'); });
  }

  function story() {
    elapsed = 0;
    claude.clear();
    codex.clear();
    shell.clear();
    agents = { claude: 'working', codex: 'working', gemini: 'waiting' };
    files = BASE_FILES.slice();
    renderFiles();
    Object.keys(agents).forEach(function (name) { setAgent(name, agents[name]); });
    mock.querySelector('[data-diff]').hidden = true;
    queue = [];
    typing = [];
    spinner = null;

    claude.line('<span class="t-box claude"><span class="t-claude">✻</span> Welcome to <span class="t-b">Claude Code</span>\n<span class="t-dim">  cwd: ~/Dev/vibra</span></span>');
    codex.line('<span class="t-box"><span class="t-b">&gt;_ OpenAI Codex</span>\n<span class="t-dim">directory: ~/Dev/vibra</span></span>');
    shell.line('<span class="t-ok">~/Dev/vibra</span> <span class="t-acc">main</span>');
    shell.line('<span class="t-claude">❯</span> cargo watch -x check');

    at(300, function () { type(claude, '<span class="t-dim">&gt;</span> ', 'fix the failing auth test', 28); });
    at(400, function () {
      shell.line('<span class="t-dim">[Running \'cargo check\']</span>');
      shell.line('    <span class="t-ok t-b">Finished</span> `dev` profile in 0.84s');
    });
    at(500, function () { type(codex, '<span class="t-dim">›</span> ', 'add a dark mode toggle to Settings', 24); });
    at(1500, function () {
      spinner = claude.line('');
      claude.tail = spinner;
      spinner.start = elapsed;
      claude.line('<span class="t-claude">⏺</span> <span class="t-b">Read</span>(src/auth/session.rs)');
    });
    at(1900, function () { claude.line('  <span class="t-dim">⎿  Read 182 lines</span>'); });
    at(1900, function () { codex.line('<span class="t-dim">•</span> Explored'); });
    at(2200, function () { codex.line('  <span class="t-dim">└ Search theme in src/ui</span>'); });
    at(2700, function () {
      claude.line('<span class="t-claude">⏺</span> The test reads the real clock and sometimes runs before the token refresh. I’ll inject the clock instead.');
    });
    at(3100, function () {
      codex.line('<span class="t-dim">•</span> Edited <span class="t-b">src/ui/settings/appearance.rs</span> (<span class="t-ok">+38</span> <span class="t-err">−2</span>)');
      addFile({ name: 'appearance.rs', dir: 'src/ui/settings', add: 38, del: 2, letter: 'M' });
    });
    recheck(3300);
    at(4000, function () { claude.line('<span class="t-claude">⏺</span> <span class="t-b">Update</span>(src/auth/session.rs)'); });
    at(4400, function () {
      claude.line('  <span class="t-dim">⎿  Updated with</span> <span class="t-ok">12 additions</span> <span class="t-dim">and</span> <span class="t-err">4 removals</span>');
      addFile({ name: 'session.rs', dir: 'src/auth', add: 12, del: 4, letter: 'M' });
      mock.querySelector('[data-diff]').hidden = false;
    });
    recheck(4700);
    at(5000, function () {
      codex.line('<span class="t-box ask"><span class="t-err t-b">Allow command?</span>\n<span class="t-dim">$</span> cargo build --release\n<span class="t-acc">›</span> Yes   <span class="t-dim">Always   No</span></span>');
      setAgent('codex', 'permission');
    });
    at(5600, function () { claude.line('<span class="t-claude">⏺</span> <span class="t-b">Bash</span>(cargo test auth)'); });
    at(6600, function () { claude.line('  <span class="t-dim">⎿</span>  <span class="t-ok">test result: ok.</span> 24 passed; 0 failed'); });
    at(7600, function () {
      spinner.remove();
      spinner = null;
      claude.tail = null;
      claude.line('<span class="t-claude">⏺</span> Done: the test no longer depends on real time.');
      claude.line('');
      claude.line('<span class="t-dim">&gt;</span> <span class="t-caret"></span>');
      setAgent('claude', 'idle');
    });
  }

  var SPIN = '·✢✳✶✻✽✻✶✳✢';
  function advance(dt) {
    var target = elapsed + dt;
    if (target >= LOOP) return story();
    // Steps run at their own time, so text they start typing is timed from there.
    while (queue.length && queue[0].at <= target) {
      var step = queue.shift();
      elapsed = Math.max(elapsed, step.at);
      step.run();
    }
    elapsed = target;
    typing = typing.filter(function (task) {
      var n = Math.min(task.text.length, Math.floor((elapsed - task.start) / task.speed));
      task.el.textContent = task.text.slice(0, n);
      return n < task.text.length;
    });
    if (spinner) {
      var t = elapsed - spinner.start;
      spinner.innerHTML = '<span class="t-claude">' + SPIN[Math.floor(t / 110) % SPIN.length] +
        ' Fixing the flaky test…</span> <span class="t-dim">(' + Math.floor(t / 1000) + 's · esc to interrupt)</span>';
    }
  }

  story();
  if (calm) {
    // Show the scene as it stands once the agents are done.
    advance(9000);
  } else {
    var frameId = 0, lastFrame = 0;
    var tickMock = function (now) {
      frameId = requestAnimationFrame(tickMock);
      advance(Math.min(100, now - lastFrame));
      lastFrame = now;
    };
    whileVisible(mock, function () {
      cancelAnimationFrame(frameId);
      lastFrame = performance.now();
      frameId = requestAnimationFrame(tickMock);
    }, function () {
      cancelAnimationFrame(frameId);
    });
  }

  /* ---------- Feature cards ---------- */

  // Agents: statuses move on, and a notification arrives for the pane you are not watching.
  (function () {
    var viz = document.querySelector('[data-viz="agents"]');
    var cards = all('.m-card', viz);
    var toast = viz.querySelector('[data-toast]');
    var NAMES = ['claude', 'codex', 'gemini', 'grok'];
    var START = { claude: 'working', codex: 'working', gemini: 'waiting', grok: 'idle' };
    var SCENES = [
      { toast: null },
      { codex: 'permission', toast: ['Codex needs permission', 'vibra · Dark mode toggle'] },
      { toast: null },
      { claude: 'idle', toast: ['Claude finished', 'vibra · Fix flaky auth test'] },
      { codex: 'working', gemini: 'working', toast: null },
      { grok: 'working', gemini: 'idle' }
    ];
    var state = {};
    cycle(viz, 2400, function (n) {
      var scene = SCENES[n % SCENES.length];
      if (n % SCENES.length === 0) Object.keys(START).forEach(function (key) { state[key] = START[key]; });
      NAMES.forEach(function (name, i) {
        if (scene[name]) state[name] = scene[name];
        setState(cards[i], state[name]);
      });
      if (scene.toast) {
        toast.querySelector('[data-toast-title]').textContent = scene.toast[0];
        toast.querySelector('[data-toast-body]').textContent = scene.toast[1];
      }
      toast.classList.toggle('show', !!scene.toast);
    });
  })();

  // Panes: box-drawing layouts that split, zoom and restore.
  (function () {
    var pre = document.querySelector('[data-panes]');
    var key = document.querySelector('[data-panes-key]');
    var COLS = 31, ROWS = 10;
    var FRAMES = [
      ['⌘T  new tab', [[0, 0, 30, 9, 'claude', 1]]],
      ['drag a tab → split', [[0, 0, 15, 9, 'claude'], [15, 0, 30, 9, 'codex', 1]]],
      ['four directions', [[0, 0, 15, 9, 'claude'], [15, 0, 30, 5, 'codex'], [15, 5, 30, 9, 'zsh', 1]]],
      ['⇧⌘↵  zoom the pane', [[0, 0, 30, 9, 'zsh', 1]]],
      ['⇧⌘↵  restore it', [[0, 0, 15, 9, 'claude'], [15, 0, 30, 5, 'codex'], [15, 5, 30, 9, 'zsh', 1]]]
    ];
    var GLYPH = { 5: '│', 10: '─', 6: '┌', 12: '┐', 3: '└', 9: '┘', 7: '├', 13: '┤', 14: '┬', 11: '┴', 15: '┼' };

    // Each border cell records which way its lines run (up 1, right 2, down 4, left 8).
    function render(panes) {
      var bits = [], cells = [], i;
      for (i = 0; i < COLS * ROWS; i++) bits[i] = 0;
      function link(x, y, m) { bits[y * COLS + x] |= m; }
      panes.forEach(function (p) {
        for (var x = p[0]; x < p[2]; x++) {
          link(x, p[1], 2); link(x + 1, p[1], 8);
          link(x, p[3], 2); link(x + 1, p[3], 8);
        }
        for (var y = p[1]; y < p[3]; y++) {
          link(p[0], y, 4); link(p[0], y + 1, 1);
          link(p[2], y, 4); link(p[2], y + 1, 1);
        }
      });
      for (i = 0; i < bits.length; i++) cells[i] = GLYPH[bits[i]] || ' ';
      panes.forEach(function (p) {
        var at = (p[1] + 1) * COLS + p[0] + 2;
        var label = p[5] ? '<i>●</i> <b>' + p[4] + '</b>' : '● ' + p[4];
        cells[at] = label;
        for (var n = 1; n < p[4].length + 2; n++) cells[at + n] = '';
        if (p[5]) cells[at + COLS * 2] = '<i>▍</i>';
      });
      var lines = [];
      for (var row = 0; row < ROWS; row++) lines.push(cells.slice(row * COLS, row * COLS + COLS).join(''));
      return lines.join('\n');
    }

    cycle(pre, 1900, function (n) {
      var frame = FRAMES[(calm ? 2 : n) % FRAMES.length];
      pre.innerHTML = render(frame[1]);
      key.textContent = frame[0];
    });
  })();

  // Review: the selection walks down the list.
  (function () {
    var list = document.querySelector('[data-review]');
    var rows = all('.review-row', list);
    cycle(list, 2200, function (n) {
      rows.forEach(function (row, i) { row.classList.toggle('is-selected', i === n % 3); });
    });
  })();

  // Changes: ✨ drafts the commit message.
  (function () {
    var box = document.querySelector('[data-commit-text]');
    var message = 'fix(auth): inject the clock into token refresh';
    var placeholder = '<span class="placeholder">Message</span>';
    if (calm) {
      box.textContent = message;
      return;
    }
    var shown = 0, timer = 0;
    function step() {
      if (shown === 0) box.innerHTML = placeholder;
      if (shown < message.length) {
        box.textContent = message.slice(0, ++shown);
        timer = setTimeout(step, shown === 1 ? 900 : 32);
      } else {
        shown = 0;
        timer = setTimeout(function () {
          box.innerHTML = placeholder;
          timer = setTimeout(step, 900);
        }, 3200);
      }
    }
    whileVisible(box, function () {
      clearTimeout(timer);
      timer = setTimeout(step, 600);
    }, function () {
      clearTimeout(timer);
    });
  })();

  // Keyboard: one shortcut at a time is pressed.
  (function () {
    var keys = document.querySelector('[data-keys]');
    var rows = all('div', keys);
    if (calm) return;
    cycle(keys, 1300, function (n) {
      var row = rows[n % rows.length];
      row.classList.add('pressed');
      setTimeout(function () { row.classList.remove('pressed'); }, 380);
    });
  })();

  /* ---------- Changelog ---------- */

  (function () {
    var log = document.querySelector('[data-log]');
    var more = document.querySelector('[data-log-more]');
    var FIRST = 4, PAGE = 8;
    var dateFormat = new Intl.DateTimeFormat('en', { day: 'numeric', month: 'short', year: 'numeric', timeZone: 'UTC' });

    function text(url) {
      return fetch(url, { cache: 'no-cache' }).then(function (response) {
        if (!response.ok) throw new Error(response.status);
        return response.text();
      });
    }

    // The deploy publishes the changelog beside the page; a local preview reads it from GitHub.
    text('changelog.md')
      .catch(function () { return text('https://raw.githubusercontent.com/rubenrca/Vibra/main/CHANGELOG.md'); })
      .then(function (markdown) {
        var releases = parse(markdown);
        if (!releases.length) throw new Error('empty changelog');
        render(releases);
      })
      .catch(function () {
        log.innerHTML = '<article class="release"><div></div><ul><li>The release notes didn’t load. ' +
          'They’re all on <a href="https://github.com/rubenrca/Vibra/releases">GitHub Releases</a>.</li></ul></article>';
      });

    function parse(markdown) {
      var releases = [], current = null;
      markdown.split('\n').forEach(function (line) {
        var head = /^##\s+(\S+)\s+—\s+(\d{4})-(\d{2})-(\d{2})/.exec(line);
        if (head) {
          current = { version: head[1], date: Date.UTC(+head[2], head[3] - 1, +head[4]), notes: [] };
          releases.push(current);
        } else if (current && /^- /.test(line)) {
          current.notes.push(line.slice(2).trim());
        } else if (current && current.notes.length && /^\s+\S/.test(line)) {
          current.notes[current.notes.length - 1] += ' ' + line.trim();
        }
      });
      return releases;
    }

    function inline(note) {
      return note
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/`([^`]+)`/g, '<code>$1</code>')
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
    }

    function article(release, index) {
      var iso = new Date(release.date).toISOString().slice(0, 10);
      return '<article class="release"><div><h3 class="release-version">v' + inline(release.version) +
        (index === 0 ? ' <span class="release-latest">Latest</span>' : '') + '</h3><time datetime="' + iso + '">' +
        dateFormat.format(release.date) + '</time></div><ul>' +
        release.notes.map(function (note) { return '<li>' + inline(note) + '</li>'; }).join('') + '</ul></article>';
    }

    function render(releases) {
      var shown = Math.min(FIRST, releases.length);
      log.innerHTML = releases.slice(0, shown).map(article).join('');
      more.hidden = shown >= releases.length;
      more.onclick = function () {
        var next = Math.min(shown + PAGE, releases.length);
        log.insertAdjacentHTML('beforeend', releases.slice(shown, next).map(function (release, i) {
          return article(release, shown + i);
        }).join(''));
        shown = next;
        more.hidden = shown >= releases.length;
      };
    }
  })();
})();
