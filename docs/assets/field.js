// An ASCII field: each square of a fixed grid takes a glyph and a tile from
// the brightness and color of whatever `paint` draws beneath it.
(function () {
  var RAMP = "'.,:;otx%S#X@8";           // faintest to densest
  var DASH = RAMP.length;                // atlas column for the drifting dashes
  var INKS = ['#efece4', '#13110d', '#f2a65f', '#8bbcee', '#ec8fab', '#a3d29c'];
  var MONO = '"JetBrains Mono", ui-monospace, "SF Mono", Menlo, monospace';
  var SPARSE = 0.24, TILE = 0.32;        // below SPARSE cells thin out; above TILE they get a tile
  var SUB = 2, PAD = 4;                  // samples per cell; spare cells around the canvas
  var LIFE = 2.6, SPEED = 30;            // pulse rings: seconds alive, cells per second
  var motion = window.matchMedia('(prefers-reduced-motion: reduce)');

  // options:
  //   paint(g, W, H, cell, k)  draws the picture in CSS pixels and returns its focus {x, y}
  //   lo, hi                   luminance that maps to an empty and a full cell
  //   sharpen                  how much to boost local contrast in the picture
  //   schools                  cells per school of drifting dashes, or 0 for none
  function VibraField(canvas, options) {
    var ctx = canvas.getContext('2d');
    var lo = options.lo, span = options.hi - options.lo;
    var W = 0, H = 0, DPR = 1, CELL = 0, COLS = 0, ROWS = 0, AP = 0, atlas;
    var bufW, bufH, lum, rgb, hit = 0;
    var seedA, seedB, delay, dens;
    var focus = { x: 0, y: 0 };
    var pointer = { x: -99, y: -99, on: 0, k: 0, nx: 0, ny: 0, ex: 0, ey: 0 };
    var pulses = [], schools = [];
    var clock = 0, last = 0, raf = 0, visible = false, ready = false;

    function layout() {
      W = canvas.clientWidth;
      H = canvas.clientHeight;
      if (!W || !H) return;
      DPR = Math.min(window.devicePixelRatio || 1, 2);
      CELL = W < 700 ? 9 : Math.max(11, Math.min(16, Math.round(W / 115)));
      COLS = Math.ceil(W / CELL);
      ROWS = Math.ceil(H / CELL);
      canvas.width = Math.round(W * DPR);
      canvas.height = Math.round(H * DPR);
      canvas.style.setProperty('--cell', CELL + 'px');
      buildAtlas();
      sample();

      var n = COLS * ROWS;
      var far = Math.hypot(Math.max(focus.x, COLS - focus.x), Math.max(focus.y, ROWS - focus.y));
      seedA = new Float32Array(n);
      seedB = new Float32Array(n);
      delay = new Float32Array(n);
      dens = new Float32Array(n);
      for (var i = 0; i < n; i++) {
        seedA[i] = Math.random();
        seedB[i] = Math.random();
        // Cells decode outward from the focus.
        delay[i] = 0.15 + Math.hypot(i % COLS - focus.x, (i / COLS | 0) - focus.y) / far * 1.1 + seedA[i] * 0.35;
      }
      schools = [];
      if (options.schools) {
        for (var s = Math.max(2, Math.round(n / options.schools)); s > 0; s--) schools.push(school(true));
      }
      ready = true;
    }

    // Glyphs are drawn once per ink into an atlas; per frame they are only copied.
    function buildAtlas() {
      if (!CELL) return;
      AP = Math.round(CELL * DPR);
      atlas = document.createElement('canvas');
      atlas.width = AP * (RAMP.length + 1);
      atlas.height = AP * INKS.length;
      var a = atlas.getContext('2d');
      var glyphs = RAMP + '-';
      a.font = '500 ' + Math.round(CELL * DPR * 0.78) + 'px ' + MONO;
      a.textAlign = 'center';
      a.textBaseline = 'middle';
      for (var row = 0; row < INKS.length; row++) {
        a.fillStyle = INKS[row];
        for (var k = 0; k < glyphs.length; k++) a.fillText(glyphs[k], (k + 0.5) * AP, (row + 0.5) * AP);
      }
    }

    function sample() {
      bufW = (COLS + PAD * 2) * SUB;
      bufH = (ROWS + PAD * 2) * SUB;
      var k = SUB / CELL;
      var buf = document.createElement('canvas');
      buf.width = bufW;
      buf.height = bufH;
      var g = buf.getContext('2d', { willReadFrequently: true });
      g.setTransform(k, 0, 0, k, PAD * SUB, PAD * SUB);
      var at = options.paint(g, W, H, CELL, k);
      focus.x = at.x / CELL;
      focus.y = at.y / CELL;

      var data = g.getImageData(0, 0, bufW, bufH).data;
      lum = new Float32Array(bufW * bufH);
      rgb = new Uint8Array(bufW * bufH * 3);
      for (var i = 0, j = 0; i < lum.length; i++, j += 4) {
        lum[i] = (0.2126 * data[j] + 0.7152 * data[j + 1] + 0.0722 * data[j + 2]) / 255 * (data[j + 3] / 255);
        rgb[i * 3] = data[j];
        rgb[i * 3 + 1] = data[j + 1];
        rgb[i * 3 + 2] = data[j + 2];
      }
      if (options.sharpen) {
        // Specks and edges are what make neighboring glyphs differ.
        var soft = boxBlur(lum, bufW, bufH, 2);
        for (i = 0; i < lum.length; i++) lum[i] = Math.max(0, lum[i] + (lum[i] - soft[i]) * options.sharpen);
      }
    }

    // Bilinear luminance at a point in cell units; leaves the sample index in `hit` for color.
    function luminance(u, v) {
      var x = (u + PAD) * SUB - 0.5, y = (v + PAD) * SUB - 0.5;
      if (x < 0) x = 0; else if (x > bufW - 1.001) x = bufW - 1.001;
      if (y < 0) y = 0; else if (y > bufH - 1.001) y = bufH - 1.001;
      var x0 = x | 0, y0 = y | 0, fx = x - x0, fy = y - y0, i = y0 * bufW + x0;
      var top = lum[i] + (lum[i + 1] - lum[i]) * fx;
      var bottom = lum[i + bufW] + (lum[i + bufW + 1] - lum[i + bufW]) * fx;
      hit = i;
      return top + (bottom - top) * fy;
    }

    function draw(t, dt) {
      var calm = motion.matches;
      pointer.k += ((calm ? 0 : pointer.on) - pointer.k) * 0.1;
      pointer.ex += (pointer.nx - pointer.ex) * 0.04;
      pointer.ey += (pointer.ny - pointer.ey) * 0.04;
      // The picture breathes and drifts a little, and leans away from the pointer.
      var zoom = calm ? 1 : 1 + 0.018 * Math.sin(t * 0.3);
      var ox = calm ? 0 : Math.sin(t * 0.19) * 0.7 - pointer.ex * 1.4;
      var oy = calm ? 0 : Math.sin(t * 0.23) * 0.5 - pointer.ey * 0.9;
      var tick = (t * 9) | 0, reach = 7;
      pulses = pulses.filter(function (p) { return t - p.t < LIFE; });

      ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
      ctx.clearRect(0, 0, W, H);

      for (var cy = 0; cy < ROWS; cy++) {
        for (var cx = 0; cx < COLS; cx++) {
          var i = cy * COLS + cx, s = seedA[i];
          dens[i] = 0;
          var reveal = (t - delay[i]) / 0.45;
          if (reveal <= 0) continue;

          var u = focus.x + (cx + 0.5 - focus.x) / zoom + ox;
          var v = focus.y + (cy + 0.5 - focus.y) / zoom + oy;
          var wave = 0;
          for (var k = 0; k < pulses.length; k++) {
            var p = pulses[k], age = t - p.t;
            if (age < 0) continue;
            var px = cx - p.x, py = cy - p.y;
            var band = (Math.sqrt(px * px + py * py) - age * SPEED) / 2.4;
            if (band > -3 && band < 3) wave += Math.exp(-band * band) * p.amp * (1 - age / LIFE);
          }
          // A passing ring shakes the cells it crosses sideways.
          if (wave) u += (s - 0.5) * wave * 3;

          var d = (luminance(u, v) - lo) / span + wave * 0.5;
          if (d > 0.3 && !calm) d += 0.12 * Math.sin(t * (1 + s * 2.4) + s * 50);
          var near = 0;
          if (pointer.k > 0.01) {
            var dx = cx - pointer.x, dy = cy - pointer.y, q = Math.sqrt(dx * dx + dy * dy);
            if (q < reach) {
              near = (1 - q / reach) * pointer.k;
              d += near * 0.32;
            }
          }
          if (reveal < 1) d *= reveal;
          if (d < 0.04) continue;
          // Faint cells thin out so the background breaks into scattered marks, not haze.
          if (d < SPARSE && seedB[i] > (d / SPARSE) * (d / SPARSE)) continue;
          if (d > 1) d = 1;
          dens[i] = d;

          var glyph = (d * RAMP.length * 0.999) | 0, roll = hash(i, tick);
          if (reveal < 1 || near > 0.35) glyph = (roll * RAMP.length) | 0;
          else if (roll < 0.025) glyph = Math.min(RAMP.length - 1, Math.max(0, glyph + (s < 0.5 ? -1 : 1)));

          var c = hit * 3, r = rgb[c], g = rgb[c + 1], b = rgb[c + 2];
          var x = cx * CELL, y = cy * CELL, ink = 0, alpha, light = 0;
          if (d > TILE) {
            // Tile lightness follows density; its hue is the picture's, muted.
            var tl = (d - TILE) / (1 - TILE);
            light = 24 + tl * 166;
            var m = light / Math.max(12, 0.2126 * r + 0.7152 * g + 0.0722 * b);
            ctx.globalAlpha = 1;
            ctx.fillStyle = 'rgb(' + tint(light, r * m) + ',' + tint(light, g * m) + ',' + tint(light, b * m) + ')';
            ctx.fillRect(x + 0.5, y + 0.5, CELL - 1, CELL - 1);
            if (light > 128) {
              ink = 1;
              alpha = 0.82;
            } else {
              alpha = 0.42 + tl * 0.55;
            }
          } else {
            alpha = 0.14 + d * 1.7;
          }
          if (seedB[i] > 0.935 && d > 0.24 && light < 128) {
            ink = (r > b ? 2 : 3) + (s > 0.65 ? 2 : 0);
            alpha = 0.95;
          }
          ctx.globalAlpha = alpha > 1 ? 1 : alpha;
          ctx.drawImage(atlas, glyph * AP, ink * AP, AP, AP, x, y, CELL, CELL);
        }
      }
      drawSchools(t, dt);
      ctx.globalAlpha = 1;
    }

    // Loose schools of dashes drift across the empty parts of the grid.
    function school(anywhere) {
      var dir = Math.random() < 0.5 ? -1 : 1, members = [];
      for (var n = 5 + ((Math.random() * 8) | 0); n > 0; n--) {
        members.push({ dx: (Math.random() - 0.5) * 7, dy: (Math.random() - 0.5) * 3, f: 0.5 + Math.random() });
      }
      return {
        x: anywhere ? Math.random() * COLS : dir > 0 ? -8 : COLS + 8,
        y: Math.random() * ROWS,
        vx: dir * (1.5 + Math.random() * 2.5),
        a: 0.16 + Math.random() * 0.18,
        ph: Math.random() * 6.3,
        members: members
      };
    }

    function drawSchools(t, dt) {
      for (var k = 0; k < schools.length; k++) {
        var sc = schools[k];
        sc.x += sc.vx * dt;
        if (sc.x < -12 || sc.x > COLS + 12) {
          schools[k] = school(false);
          continue;
        }
        var y = sc.y + Math.sin(t * 0.4 + sc.ph) * 1.5;
        ctx.globalAlpha = sc.a * Math.min(1, t / 2);
        for (var n = 0; n < sc.members.length; n++) {
          var mb = sc.members[n];
          var mx = Math.round(sc.x + mb.dx + Math.sin(t * mb.f + n) * 0.8), my = Math.round(y + mb.dy);
          if (mx < 0 || my < 0 || mx >= COLS || my >= ROWS || dens[my * COLS + mx]) continue;
          ctx.drawImage(atlas, DASH * AP, 0, AP, AP, mx * CELL, my * CELL, CELL, CELL);
        }
      }
    }

    function run() {
      cancelAnimationFrame(raf);
      if (!ready || !visible) return;
      if (motion.matches) return draw(99, 0);
      last = performance.now();
      raf = requestAnimationFrame(frame);
    }

    function frame(now) {
      raf = requestAnimationFrame(frame);
      if (now - last < 15) return; // cap high-refresh displays at about 60 fps
      var dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      // The clock only runs while the field is on screen, so it decodes when first seen.
      clock += dt;
      draw(clock, dt);
    }

    // A ring from a point in viewport coordinates; ignored when it lands off this field.
    function pulse(clientX, clientY, amp, wait) {
      if (!ready || motion.matches) return;
      var r = canvas.getBoundingClientRect();
      var x = clientX - r.left, y = clientY - r.top;
      if (x < -40 || y < -40 || x > r.width + 40 || y > r.height + 40) return;
      pulses.push({ x: x / CELL, y: y / CELL, amp: amp, t: clock + (wait || 0) });
      if (pulses.length > 6) pulses.shift();
    }

    layout();
    if (!ready) return null;
    canvas.classList.add('ready');
    pulses.push({ x: focus.x, y: focus.y, amp: 0.55, t: 1.2 });

    new IntersectionObserver(function (entries) {
      visible = entries[0].isIntersecting;
      if (visible) run();
      else cancelAnimationFrame(raf);
    }).observe(canvas);

    var resizeTimer;
    new ResizeObserver(function () {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(function () {
        if (canvas.clientWidth === W && canvas.clientHeight === H && Math.min(window.devicePixelRatio || 1, 2) === DPR) return;
        layout();
        run();
      }, 120);
    }).observe(canvas);
    motion.addEventListener('change', run);
    // If the mono font arrived late, redraw the glyphs with it.
    if (document.fonts) document.fonts.ready.then(buildAtlas);

    window.addEventListener('pointermove', function (e) {
      if (e.pointerType === 'touch') return;
      var r = canvas.getBoundingClientRect();
      var x = e.clientX - r.left, y = e.clientY - r.top;
      var inside = x >= 0 && y >= 0 && x < r.width && y < r.height;
      pointer.on = inside ? 1 : 0;
      pointer.x = x / CELL;
      pointer.y = y / CELL;
      // Only a pointer over the field tilts the picture. Measured from elsewhere on
      // the page it is thousands of pixels away and would push the picture off center.
      pointer.nx = inside ? x / r.width * 2 - 1 : 0;
      pointer.ny = inside ? y / r.height * 2 - 1 : 0;
    }, { passive: true });
    document.documentElement.addEventListener('pointerleave', function () {
      pointer.on = 0;
      pointer.nx = pointer.ny = 0;
    });
    document.addEventListener('vibra:pulse', function (e) {
      pulse(e.detail.x, e.detail.y, e.detail.amp);
    });
    canvas.parentElement.addEventListener('pointerdown', function (e) {
      if (!e.target.closest('a, button')) pulse(e.clientX, e.clientY, 0.6);
    });

    return { pulse: pulse };
  }

  // Halve an image until it is near the target width: a single large
  // downscale aliases into noise.
  VibraField.shrink = function (src, tw) {
    var w = src.naturalWidth || src.width, h = src.naturalHeight || src.height;
    while (w / 2 >= tw) {
      var c = document.createElement('canvas');
      c.width = w = Math.ceil(w / 2);
      c.height = h = Math.ceil(h / 2);
      c.getContext('2d').drawImage(src, 0, 0, w, h);
      src = c;
    }
    return src;
  };

  function boxBlur(src, w, h, r) {
    var across = new Float32Array(src.length), out = new Float32Array(src.length);
    for (var pass = 0; pass < 2; pass++) {
      var from = pass ? across : src, to = pass ? out : across;
      var len = pass ? h : w, step = pass ? w : 1;
      for (var line = 0; line < (pass ? w : h); line++) {
        var base = pass ? line : line * w;
        for (var a = 0; a < len; a++) {
          var sum = 0, n = 0;
          for (var b = Math.max(0, a - r); b <= Math.min(len - 1, a + r); b++, n++) sum += from[base + b * step];
          to[base + a * step] = sum / n;
        }
      }
    }
    return out;
  }

  function hash(a, b) {
    var h = Math.imul(a, 0x27d4eb2d) ^ Math.imul(b, 0x165667b1);
    h = Math.imul(h ^ (h >>> 15), 0x85ebca6b);
    h ^= h >>> 13;
    return (h >>> 0) / 4294967296;
  }

  function tint(base, channel) {
    return (base + (Math.min(255, channel) - base) * 0.55) | 0;
  }

  window.VibraField = VibraField;
})();
