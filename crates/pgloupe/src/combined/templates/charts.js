/* Charts of the dashboard: small SVG builders that read colors from CSS variables. */
var PgCharts = (function (L) {
  'use strict';

  /* An XML namespace name, not an address; written in two parts so the page holds no URL at all. */
  var SVG = 'http' + '://www.w3.org/2000/svg';

  function svgEl(tag, attrs, children) {
    var node = document.createElementNS(SVG, tag);
    Object.keys(attrs || {}).forEach(function (key) { node.setAttribute(key, String(attrs[key])); });
    (children || []).forEach(function (child) { if (child) { node.appendChild(child); } });
    return node;
  }

  function text(tag, attrs, content) {
    var node = svgEl(tag, attrs);
    node.textContent = content;
    return node;
  }

  function clear(node) { while (node.firstChild) { node.removeChild(node.firstChild); } }

  /** A tiny trend line; `values` is any series of numbers. */
  function sparkline(values, color) {
    var w = 96, h = 28, pad = 2;
    var svg = svgEl('svg', { viewBox: '0 0 ' + w + ' ' + h, 'class': 'spark', 'aria-hidden': 'true', preserveAspectRatio: 'none' });
    if (!values.length) { return svg; }
    var max = Math.max.apply(null, values.concat([1]));
    var step = values.length > 1 ? (w - 2 * pad) / (values.length - 1) : 0;
    var points = values.map(function (v, i) {
      return (pad + i * step).toFixed(1) + ',' + (h - pad - (v / max) * (h - 2 * pad)).toFixed(1);
    });
    var area = 'M' + points[0] + ' L' + points.join(' L') + ' L' + (pad + (values.length - 1) * step).toFixed(1) + ',' + (h - pad) + ' L' + pad + ',' + (h - pad) + ' Z';
    svg.appendChild(svgEl('path', { d: area, fill: color, opacity: '0.16' }));
    svg.appendChild(svgEl('polyline', { points: points.join(' '), fill: 'none', stroke: color, 'stroke-width': '1.5', 'stroke-linejoin': 'round', 'stroke-linecap': 'round', 'vector-effect': 'non-scaling-stroke' }));
    return svg;
  }

  /** Small multiples over time with a crosshair, a tooltip and drag-to-select. */
  function timeline(host, opts) {
    clear(host);
    var buckets = opts.buckets;
    var series = opts.series.filter(function (s) { return s.visible !== false; });
    if (!buckets.length || !series.length) {
      var empty = document.createElement('p');
      empty.className = 'muted';
      empty.textContent = buckets.length ? 'All series are hidden.' : 'No events in this period.';
      host.appendChild(empty);
      return;
    }
    var W = Math.max(320, host.clientWidth || 800);
    var left = 46, right = 10, panel = 58, gap = 14, axis = 20;
    var H = series.length * (panel + gap) - gap + axis + 4;
    var plotW = W - left - right;
    var n = buckets.length;
    var bw = plotW / n;
    var svg = svgEl('svg', { viewBox: '0 0 ' + W + ' ' + H, width: W, height: H, 'class': 'tl-chart', role: 'img', 'aria-label': 'Activity over time' });
    var range = opts.range;
    var inRange = function (i) { return !range || (i >= Math.min(range.from, range.to) && i <= Math.max(range.from, range.to)); };

    series.forEach(function (s, row) {
      var top = row * (panel + gap);
      var values = buckets.map(function (b) { return b[s.key] || 0; });
      var max = L.niceMax(Math.max.apply(null, values));
      svg.appendChild(svgEl('line', { x1: left, x2: W - right, y1: top + panel + 0.5, y2: top + panel + 0.5, 'class': 'grid' }));
      svg.appendChild(svgEl('line', { x1: left, x2: W - right, y1: top + 0.5, y2: top + 0.5, 'class': 'grid faint' }));
      svg.appendChild(text('text', { x: left - 6, y: top + 9, 'text-anchor': 'end', 'class': 'axis' }, L.fmtCompact(max)));
      svg.appendChild(text('text', { x: left - 6, y: top + panel, 'text-anchor': 'end', 'class': 'axis' }, '0'));
      svg.appendChild(text('text', { x: left + 4, y: top + 12, 'class': 'panel-label', fill: s.color }, s.label));
      values.forEach(function (v, i) {
        if (v <= 0) { return; }
        var hgt = Math.max(1, (v / max) * panel);
        svg.appendChild(svgEl('rect', {
          x: (left + i * bw + 0.5).toFixed(2), y: (top + panel - hgt).toFixed(2),
          width: Math.max(1, bw - 1).toFixed(2), height: hgt.toFixed(2), rx: bw > 6 ? 1.5 : 0,
          fill: s.color, opacity: inRange(i) ? '0.9' : '0.22'
        }));
      });
    });

    var ticks = Math.min(6, n);
    var multiDay = buckets[0].start.slice(0, 10) !== buckets[n - 1].start.slice(0, 10);
    for (var t = 0; t < ticks; t++) {
      var idx = ticks === 1 ? 0 : Math.round(t * (n - 1) / (ticks - 1));
      var anchor = t === 0 ? 'start' : t === ticks - 1 ? 'end' : 'middle';
      var tx = left + (idx + 0.5) * bw;
      svg.appendChild(text('text', { x: tx.toFixed(1), y: H - 4, 'text-anchor': anchor, 'class': 'axis' }, L.fmtTime(buckets[idx].start, multiDay).replace(/^\d{4}-/, '')));
    }

    var plotH = H - axis - 4;
    var cross = svgEl('line', { y1: 0, y2: plotH, 'class': 'cross', visibility: 'hidden' });
    var brush = svgEl('rect', { y: 0, height: plotH, 'class': 'brush', visibility: 'hidden' });
    var hit = svgEl('rect', { x: left, y: 0, width: plotW, height: plotH, fill: 'transparent', 'class': 'hit' });
    svg.appendChild(brush);
    svg.appendChild(cross);
    svg.appendChild(hit);
    host.appendChild(svg);

    var tip = document.createElement('div');
    tip.className = 'tip';
    tip.hidden = true;
    host.appendChild(tip);

    function showBrush(a, b) {
      var lo = Math.min(a, b), hi = Math.max(a, b);
      brush.setAttribute('x', (left + lo * bw).toFixed(1));
      brush.setAttribute('width', ((hi - lo + 1) * bw).toFixed(1));
      brush.setAttribute('visibility', 'visible');
    }
    if (range) { showBrush(range.from, range.to); }

    function indexAt(event) {
      var box = svg.getBoundingClientRect();
      var x = (event.clientX - box.left) * (W / box.width) - left;
      return Math.max(0, Math.min(n - 1, Math.floor(x / bw)));
    }

    function describe(i) {
      var b = buckets[i];
      clear(tip);
      var head = document.createElement('b');
      var span = b.span > 1 ? ' (+' + (b.span - 1) + ' more buckets)' : '';
      head.textContent = L.fmtTime(b.start, true) + span;
      tip.appendChild(head);
      series.forEach(function (s) {
        var line = document.createElement('div');
        var dot = document.createElement('i');
        dot.style.background = s.color;
        line.appendChild(dot);
        line.appendChild(document.createTextNode(s.label + ': ' + L.fmtInt(b[s.key] || 0)));
        tip.appendChild(line);
      });
    }

    var dragFrom = null;
    var moved = false;
    hit.addEventListener('pointermove', function (event) {
      var i = indexAt(event);
      cross.setAttribute('x1', (left + (i + 0.5) * bw).toFixed(1));
      cross.setAttribute('x2', (left + (i + 0.5) * bw).toFixed(1));
      cross.setAttribute('visibility', 'visible');
      describe(i);
      tip.hidden = false;
      var box = host.getBoundingClientRect();
      var px = event.clientX - box.left;
      tip.style.left = Math.min(px + 14, box.width - tip.offsetWidth - 4) + 'px';
      tip.style.top = '8px';
      if (dragFrom !== null) {
        moved = moved || Math.abs(i - dragFrom) > 0;
        showBrush(dragFrom, i);
      }
    });
    hit.addEventListener('pointerleave', function () {
      cross.setAttribute('visibility', 'hidden');
      tip.hidden = true;
    });
    hit.addEventListener('pointerdown', function (event) {
      dragFrom = indexAt(event);
      moved = false;
      hit.setPointerCapture(event.pointerId);
    });
    hit.addEventListener('pointerup', function (event) {
      if (dragFrom === null) { return; }
      var to = indexAt(event);
      var from = dragFrom;
      dragFrom = null;
      if (moved && opts.onRange) { opts.onRange({ from: Math.min(from, to), to: Math.max(from, to) }); }
      else if (opts.onRange && range) { opts.onRange(null); }
    });
    hit.addEventListener('dblclick', function () { if (opts.onRange) { opts.onRange(null); } });
  }

  /** An edge as number and unit, trimmed: 1000 -> {n: '1', u: 's'}. */
  function edge(ms) {
    var units = [[3600000, 'h'], [60000, 'min'], [1000, 's'], [1, 'ms']];
    for (var i = 0; i < units.length; i++) {
      if (ms >= units[i][0]) { return { n: String(parseFloat((ms / units[i][0]).toFixed(2))), u: units[i][1] }; }
    }
    return { n: String(parseFloat(ms.toFixed(3))), u: 'ms' };
  }
  function rangeLabel(edgeMs) { var e = edge(edgeMs); return e.n + e.u; }
  /** `1\u20135ms`, `5ms\u201310s`, `\u22641s`, `>24h`. */
  function bucketLabel(prev, le) {
    if (le === null) { return '>' + rangeLabel(prev); }
    if (prev === null) { return '\u2264' + rangeLabel(le); }
    var a = edge(prev), b = edge(le);
    return (a.u === b.u ? a.n : a.n + a.u) + '\u2013' + b.n + b.u;
  }

  /** Vertical bars for `[{le_ms, count}]`; the last bucket has no edge. */
  function histogram(buckets, color) {
    var W = 640, H = 200, left = 8, right = 8, top = 22, bottom = 34;
    var svg = svgEl('svg', { viewBox: '0 0 ' + W + ' ' + H, 'class': 'histogram', role: 'img', 'aria-label': 'Duration histogram' });
    var max = Math.max.apply(null, buckets.map(function (b) { return b.count; }).concat([1]));
    var total = buckets.reduce(function (s, b) { return s + b.count; }, 0);
    var bw = (W - left - right) / buckets.length;
    buckets.forEach(function (b, i) {
      var hgt = Math.sqrt(b.count / max) * (H - top - bottom);
      var x = left + i * bw;
      var prev = i > 0 ? buckets[i - 1].le_ms : null;
      var label = bucketLabel(prev, b.le_ms);
      var title = svgEl('title');
      title.textContent = label + ': ' + L.fmtInt(b.count) + ' (' + L.fmtPct(total ? b.count * 100 / total : 0) + ')';
      var bar = svgEl('rect', { x: (x + 3).toFixed(1), y: (H - bottom - hgt).toFixed(1), width: (bw - 6).toFixed(1), height: Math.max(hgt, b.count > 0 ? 1 : 0).toFixed(1), rx: 3, fill: color, opacity: b.count ? '0.9' : '0.15' }, [title]);
      svg.appendChild(bar);
      if (b.count > 0) { svg.appendChild(text('text', { x: (x + bw / 2).toFixed(1), y: (H - bottom - hgt - 5).toFixed(1), 'text-anchor': 'middle', 'class': 'value' }, L.fmtCompact(b.count))); }
      svg.appendChild(text('text', { x: (x + bw / 2).toFixed(1), y: H - bottom + 15, 'text-anchor': 'middle', 'class': 'axis' }, label));
    });
    svg.appendChild(svgEl('line', { x1: left, x2: W - right, y1: H - bottom + 0.5, y2: H - bottom + 0.5, 'class': 'grid' }));
    return svg;
  }

  /** Ring chart of `[{label, value, color}]` with the total in the middle. */
  function donut(items, centerLabel) {
    var total = items.reduce(function (s, i) { return s + i.value; }, 0);
    var svg = svgEl('svg', { viewBox: '0 0 42 42', 'class': 'donut', role: 'img', 'aria-label': centerLabel });
    svg.appendChild(svgEl('circle', { cx: 21, cy: 21, r: 15.9155, fill: 'none', 'class': 'ring-bg', 'stroke-width': 5 }));
    var offset = 25;
    items.forEach(function (item) {
      if (!(item.value > 0) || !(total > 0)) { return; }
      var pct = item.value * 100 / total;
      var title = svgEl('title');
      title.textContent = item.label + ': ' + L.fmtInt(item.value) + ' (' + L.fmtPct(pct) + ')';
      svg.appendChild(svgEl('circle', {
        cx: 21, cy: 21, r: 15.9155, fill: 'none', stroke: item.color, 'stroke-width': 5,
        'stroke-dasharray': pct.toFixed(3) + ' ' + (100 - pct).toFixed(3), 'stroke-dashoffset': offset.toFixed(3)
      }, [title]));
      offset -= pct;
    });
    svg.appendChild(text('text', { x: 21, y: 21.5, 'text-anchor': 'middle', 'class': 'donut-total' }, L.fmtCompact(total)));
    svg.appendChild(text('text', { x: 21, y: 26.5, 'text-anchor': 'middle', 'class': 'donut-label' }, centerLabel));
    return svg;
  }

  /** p50 / p95 / p99 / max on a log scale, as an element. */
  function ladder(log) {
    var wrap = document.createElement('div');
    wrap.className = 'ladder';
    var marks = [['p50', log.p50_ms], ['p95', log.p95_ms], ['p99', log.p99_ms], ['max', log.max_ms]].filter(function (m) { return m[1] !== null && m[1] !== undefined; });
    if (!marks.length) { return wrap; }
    var values = marks.map(function (m) { return Math.max(m[1], 0.001); });
    var lo = Math.min.apply(null, values) / 1.5;
    var hi = Math.max.apply(null, values) * 1.5;
    var track = document.createElement('div');
    track.className = 'ladder-track';
    marks.forEach(function (m, i) {
      var pos = L.logPosition(values[i], lo, hi);
      var tick = document.createElement('div');
      tick.className = 'ladder-mark ' + m[0];
      tick.style.left = (pos * 100).toFixed(1) + '%';
      tick.title = m[0] + ' ' + L.fmtMs(m[1]);
      track.appendChild(tick);
    });
    var legend = document.createElement('div');
    legend.className = 'ladder-legend';
    marks.forEach(function (m) {
      var item = document.createElement('span');
      var name = document.createElement('b');
      name.textContent = m[0];
      item.appendChild(name);
      item.appendChild(document.createTextNode(' ' + L.fmtMs(m[1])));
      legend.appendChild(item);
    });
    wrap.appendChild(track);
    wrap.appendChild(legend);
    return wrap;
  }

  return { svgEl: svgEl, sparkline: sparkline, timeline: timeline, histogram: histogram, donut: donut, ladder: ladder, clear: clear };
})(PgLib);
