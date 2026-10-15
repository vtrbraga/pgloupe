/* The dashboard. It reads the report embedded in the page and renders it with textContent only. */
(function () {
  'use strict';
  var L = PgLib, C = PgCharts;
  var dataNode = document.getElementById('pgloupe-data');
  var root = document.getElementById('app');
  if (!dataNode || !root) { return; }
  var rawJson = dataNode.textContent;
  var D;
  try { D = JSON.parse(rawJson); } catch (e) { return; }
  var logs = D.logs || null;
  var stats = D.inputs && D.inputs.stats;
  var rows = D.statements || [];
  var findings = D.findings || [];
  var byFp = {};
  rows.forEach(function (r) { byFp[r.fingerprint] = r; });

  var SERIES = [
    { key: 'statements', label: 'Statements', color: 'var(--c-statements)', visible: true },
    { key: 'errors', label: 'Errors', color: 'var(--c-errors)', visible: true },
    { key: 'lock_waits', label: 'Lock waits', color: 'var(--c-locks)', visible: true },
    { key: 'connections', label: 'Connections', color: 'var(--c-connections)', visible: true }
  ];
  var KIND_COLORS = { select: '#4f7cff', insert: '#12a594', update: '#f0a020', delete: '#e5484d', copy: '#8b5cf6', ddl: '#d946ef', tcl: '#64748b', utility: '#94a3b8', other: '#cbd5e1' };

  var state = L.parseHash(location.hash);
  var display = logs ? L.downsample(logs.timeline.buckets, 240) : [];

  /* ---------- helpers ---------- */
  function h(tag, attrs, kids) {
    var node = document.createElement(tag);
    Object.keys(attrs || {}).forEach(function (key) {
      var v = attrs[key];
      if (v === null || v === undefined || v === false) { return; }
      if (key === 'class') { node.className = v; }
      else if (key === 'text') { node.textContent = v; }
      else if (key.slice(0, 2) === 'on') { node.addEventListener(key.slice(2), v); }
      else if (key === 'style') { Object.keys(v).forEach(function (k) { node.style.setProperty(k, v[k]); }); }
      else if (v === true) { node.setAttribute(key, ''); }
      else { node.setAttribute(key, String(v)); }
    });
    append(node, kids);
    return node;
  }
  function append(node, kids) {
    if (kids === null || kids === undefined || kids === false) { return; }
    if (Array.isArray(kids)) { kids.forEach(function (k) { append(node, k); }); }
    else if (typeof kids === 'string' || typeof kids === 'number') { node.appendChild(document.createTextNode(String(kids))); }
    else { node.appendChild(kids); }
  }
  function store(key, value) { try { if (value === undefined) { return localStorage.getItem(key); } localStorage.setItem(key, value); } catch (e) { return null; } return null; }
  function toast(message) {
    var t = h('div', { 'class': 'toast', role: 'status', text: message });
    document.body.appendChild(t);
    setTimeout(function () { t.remove(); }, 1600);
  }
  function copy(textValue, what) {
    var done = function () { toast(what + ' copied'); };
    if (navigator.clipboard && navigator.clipboard.writeText) { navigator.clipboard.writeText(textValue).then(done, function () { toast('Copy failed'); }); }
    else { toast('Copy is not available here'); }
  }
  function badge(textValue, cls) { return h('span', { 'class': 'badge ' + (cls || ''), text: textValue }); }
  function num(textValue) { return textValue; }
  function sqlNode(sql, cls) {
    var pre = h('pre', { 'class': 'sql-block ' + (cls || '') });
    L.tokenizeSql(sql).forEach(function (tok) {
      if (tok.t === 'ws' || tok.t === 'op' || tok.t === 'id') { pre.appendChild(document.createTextNode(tok.v)); }
      else { pre.appendChild(h('span', { 'class': tok.t, text: tok.v })); }
    });
    return pre;
  }
  function panel(title, body, extra) {
    return h('section', { 'class': 'panel' }, [title ? h('h3', null, [title, extra ? h('span', { 'class': 'grow' }) : null, extra || null]) : null, body]);
  }
  function kpi(label, value, hint, spark, tone) {
    return h('div', { 'class': 'panel kpi ' + (tone || '') }, [
      h('span', { 'class': 'label', text: label }),
      h('span', { 'class': 'value', text: value }),
      h('span', { 'class': 'hint', text: hint || '' }),
      spark || null
    ]);
  }
  function series(key) { return display.map(function (b) { return b[key] || 0; }); }
  function emptyNote(message) { return h('div', { 'class': 'empty', text: message }); }
  function openStatement(fp) { setState({ fp: fp }); }

  /* ---------- state ---------- */
  var drawerCursor = null;
  function setState(patch, opts) {
    Object.keys(patch).forEach(function (k) { state[k] = patch[k]; });
    var hash = L.buildHash(state);
    try { history.replaceState(null, '', hash || location.pathname + location.search); } catch (e) { location.hash = hash; }
    if (patch.s !== undefined || (opts && opts.render)) { render(); }
    else if (patch.fp !== undefined) { drawer(); markSelected(); }
    else if (patch.from !== undefined || patch.to !== undefined) { render(); }
  }
  function theme() { return state.th || store('pgloupe.theme') || ''; }
  function applyTheme() {
    var t = theme();
    if (t === 'light' || t === 'dark') { document.documentElement.setAttribute('data-theme', t); }
    else { document.documentElement.removeAttribute('data-theme'); }
  }
  function cycleTheme() {
    var next = { '': 'light', light: 'dark', dark: '' }[theme()] || '';
    store('pgloupe.theme', next);
    state.th = '';
    applyTheme();
    renderChrome();
    toast('Theme: ' + (next || 'automatic'));
  }

  /* ---------- sections ---------- */
  var sections = [
    { id: 'overview', label: 'Overview', show: true },
    { id: 'statements', label: 'Statements', show: rows.length > 0 },
    { id: 'errors', label: 'Errors', show: !!logs },
    { id: 'locks', label: 'Locks & temp files', show: !!logs },
    { id: 'sessions', label: 'Sessions', show: !!logs },
    { id: 'maintenance', label: 'Maintenance', show: !!logs },
    { id: 'breakdowns', label: 'Breakdowns', show: !!logs },
    { id: 'slowest', label: 'Slowest', show: !!(logs && logs.slowest && logs.slowest.rows.length) },
    { id: 'findings', label: 'Findings', show: findings.length > 0 }
  ].filter(function (s) { return s.show; });
  if (!sections.some(function (s) { return s.id === state.s; })) { state.s = 'overview'; }

  var chrome = {};
  function renderChrome() {
    var warn = findings.filter(function (f) { return f.severity === 'warn'; }).length;
    var nav = h('nav', { 'aria-label': 'Sections' }, sections.map(function (s, i) {
      return h('button', {
        'class': 'nav-item', 'aria-current': s.id === state.s ? 'page' : null,
        onclick: function () { setState({ s: s.id, fp: '' }); window.scrollTo(0, 0); }
      }, [s.label, s.id === 'findings' ? badge(String(findings.length), warn ? 'warn' : 'info') : null, h('kbd', { text: String(i + 1) })]);
    }));
    var logo = h('span', { 'class': 'brand' }, [
      C.svgEl('svg', { viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': '2', 'stroke-linecap': 'round' }, [
        C.svgEl('circle', { cx: 10, cy: 10, r: 6.5 }),
        C.svgEl('line', { x1: 15, y1: 15, x2: 21, y2: 21 })
      ]),
      'pgloupe'
    ]);
    var meta = h('div', { 'class': 'meta' }, [D.generator, h('br'), 'schema ' + D.schema_version]);
    var rail = h('aside', { 'class': 'rail' }, [logo, nav, h('div', { 'class': 'spacer' }), meta]);
    var subs = [];
    if (D.windows && D.windows.logs) { subs.push(h('span', { text: 'Logs ' + L.fmtTime(D.windows.logs.from, true) + ' → ' + L.fmtTime(D.windows.logs.to, true) })); }
    if (D.windows && D.windows.stats) { subs.push(h('span', { text: 'Statistics ' + L.fmtTime(D.windows.stats.from, true) + ' → ' + L.fmtTime(D.windows.stats.to, true) })); }
    if (D.windows && D.windows.overlap_pct !== null && D.windows.overlap_pct !== undefined) { subs.push(h('span', { text: 'overlap ' + Math.round(D.windows.overlap_pct) + '%' })); }
    var themeName = theme() || 'auto';
    var top = h('div', { 'class': 'topbar' }, [
      h('div', null, [h('h1', { text: (sections.filter(function (s) { return s.id === state.s; })[0] || sections[0]).label }), h('div', { 'class': 'sub' }, subs)]),
      h('div', { 'class': 'actions' }, [
        h('button', { 'class': 'btn', title: 'Switch theme', onclick: cycleTheme, text: 'Theme: ' + themeName }),
        h('button', { 'class': 'btn', title: 'Download the report data', onclick: downloadJson, text: 'JSON' }),
        h('button', { 'class': 'btn', title: 'Print the static report', onclick: function () { window.print(); }, text: 'Print' })
      ])
    ]);
    chrome.rail = rail; chrome.top = top;
    var old = root.querySelector('.rail');
    if (old) { root.replaceChild(rail, old); } else { root.insertBefore(rail, root.firstChild); }
    var mainNode = root.querySelector('.main');
    if (!mainNode) { mainNode = h('div', { 'class': 'main', role: 'main' }); root.appendChild(mainNode); }
    var oldTop = mainNode.querySelector('.topbar');
    if (oldTop) { mainNode.replaceChild(top, oldTop); } else { mainNode.insertBefore(top, mainNode.firstChild); }
  }
  function downloadJson() {
    var blob = new Blob([rawJson], { type: 'application/json' });
    var a = h('a', { href: URL.createObjectURL(blob), download: 'pgloupe-report.json' });
    document.body.appendChild(a); a.click(); a.remove();
  }

  /* ---------- overview ---------- */
  function rangeNow() { return state.from !== null && state.to !== null && display.length ? { from: state.from, to: Math.min(state.to, display.length - 1) } : null; }
  function viewOverview() {
    var parts = [];
    var range = rangeNow();
    if (logs) {
      var sum = function (key) { return L.sumRange(display, key, range); };
      var scope = range ? 'in selection' : 'whole period';
      var cancelled = logs.cancelled ? logs.cancelled.total : 0;
      parts.push(h('div', { 'class': 'grid kpis' }, [
        kpi('Statements', L.fmtCompact(sum('statements')), scope, C.sparkline(series('statements'), 'var(--c-statements)')),
        kpi('Errors', L.fmtCompact(sum('errors')), scope, C.sparkline(series('errors'), 'var(--c-errors)'), sum('errors') > 0 ? 'bad' : ''),
        kpi('Cancelled', L.fmtInt(cancelled), cancelled ? reasonsText(logs.cancelled) : 'none', null, cancelled ? 'warn' : ''),
        kpi('Lock waits', L.fmtCompact(sum('lock_waits')), 'total wait ' + L.fmtMs(logs.locks.total_wait_ms) + ', ' + logs.locks.deadlocks + ' deadlocks', C.sparkline(series('lock_waits'), 'var(--c-locks)'), logs.locks.deadlocks ? 'warn' : ''),
        kpi('Temp files', L.fmtBytes(logs.temp_files.bytes), L.fmtInt(logs.temp_files.files) + ' files, largest ' + L.fmtBytes(logs.temp_files.max_bytes)),
        kpi('Connections', L.fmtCompact(sum('connections')), scope, C.sparkline(series('connections'), 'var(--c-connections)'))
      ]));
    }
    if (findings.length) {
      var top = findings.slice(0, 3);
      parts.push(panel('Findings', h('div', { 'class': 'findings' }, top.map(findingNode)), findings.length > 3 ? h('button', { 'class': 'btn small', onclick: function () { setState({ s: 'findings' }); }, text: 'All ' + findings.length }) : null));
    }
    if (stats) { parts.push(statsPanel()); }
    if (logs) {
      parts.push(timelinePanel());
      parts.push(h('div', { 'class': 'grid cols-2' }, [kindsPanel(), histogramPanel('Statement durations', logs.histograms.statements, 'var(--c-statements)', logs.overview.records)]));
    }
    if (!logs && !stats) { parts.push(emptyNote('Nothing to show.')); }
    return parts;
  }
  function reasonsText(c) { return c.by_reason.map(function (r) { return r.reason.replace('_', ' ') + ' ' + r.count; }).join(', '); }
  function statsPanel() {
    var c = stats.counts || {};
    var facts = [
      ['Server', 'PostgreSQL ' + Math.floor(stats.server_version_num / 10000)],
      ['Counters reset', stats.reset],
      ['Evictions', stats.evictions === null || stats.evictions === undefined ? 'unknown' : L.fmtInt(stats.evictions)],
      ['Entries', L.fmtInt(c.entries)], ['New', L.fmtInt(c['new'])], ['Gone', L.fmtInt(c.gone)], ['Restarted', L.fmtInt(c.restarted)]
    ];
    return panel('pg_stat_statements window', h('dl', { 'class': 'facts' }, facts.map(function (f) { return h('div', null, [h('dt', { text: f[0] }), h('dd', { text: String(f[1]) })]); })));
  }
  function timelinePanel() {
    var host = h('div', { 'class': 'chart-host' });
    var legend = h('div', { 'class': 'legend' }, SERIES.map(function (s) {
      return h('button', {
        'aria-pressed': String(s.visible), title: 'Show or hide ' + s.label,
        onclick: function () { s.visible = !s.visible; draw(); legend.querySelectorAll('button').forEach(function (b, i) { b.setAttribute('aria-pressed', String(SERIES[i].visible)); }); }
      }, [h('i', { style: { background: s.color } }), s.label]);
    }));
    var note = h('div', { 'class': 'range-note' });
    function draw() {
      C.timeline(host, {
        buckets: display, series: SERIES, range: rangeNow(),
        onRange: function (r) { setState(r ? { from: r.from, to: r.to } : { from: null, to: null }); }
      });
      C.clear(note);
      var r = rangeNow();
      if (r) {
        note.appendChild(document.createTextNode('Selection ' + L.fmtTime(display[r.from].start, true) + ' → ' + L.fmtTime(display[r.to].start, true) + '. Cards follow the selection; statement rows cover the whole period.'));
        note.appendChild(h('button', { 'class': 'btn small', onclick: function () { setState({ from: null, to: null }); }, text: 'Clear' }));
      } else {
        note.appendChild(document.createTextNode('Drag across the chart to select a period.'));
      }
    }
    var wrap = panel('Activity', h('div', null, [legend, host, note]));
    setTimeout(draw, 0);
    window.__redraw = draw;
    return wrap;
  }
  function kindsPanel() {
    var kinds = logs.kinds || [];
    if (!kinds.length) { return panel('Query types', emptyNote('No statements.')); }
    var items = kinds.map(function (k) { return { label: k.kind, value: k.calls, color: KIND_COLORS[k.kind] || '#94a3b8' }; });
    var total = kinds.reduce(function (s, k) { return s + k.calls; }, 0);
    var legend = h('div', { 'class': 'donut-legend' }, kinds.map(function (k) {
      return h('div', null, [h('i', { style: { background: KIND_COLORS[k.kind] || '#94a3b8' } }), k.kind, h('span', { 'class': 'n', text: L.fmtInt(k.calls) + ' · ' + L.fmtPct(k.share_pct, 0) + ' time' })]);
    }));
    return panel('Query types', h('div', { 'class': 'donut-wrap' }, [C.donut(items, 'statements'), legend]), null);
  }
  function histogramPanel(title, buckets, color) {
    var total = buckets.reduce(function (s, b) { return s + b.count; }, 0);
    if (!total) { return panel(title, emptyNote('Nothing recorded.')); }
    return panel(title, h('div', null, [C.histogram(buckets, color), h('p', { 'class': 'muted session-note', text: 'Counts near a range edge are approximate (about 1%).' })]));
  }
  function findingNode(f) {
    return h('div', { 'class': 'finding ' + f.severity }, [
      h('span', { 'class': 'dot' }),
      h('div', null, [
        h('div', null, [h('span', { 'class': 'code', text: f.code }), ' ', f.message]),
        f.statements && f.statements.length ? h('div', { 'class': 'links' }, f.statements.map(function (fp) {
          return h('button', { title: 'Open the statement', onclick: function () { openStatement(fp); }, text: L.shortFp(fp) });
        })) : null
      ])
    ]);
  }

  /* ---------- statements ---------- */
  var COLUMNS = [
    { key: 'link', label: 'Source', cell: function (r) { return badge(r.link, r.link); } },
    { key: 'statement', label: 'Statement', cell: function (r) { return h('div', { 'class': 'stmt' }, [h('span', { 'class': 'sql', text: r.text.replace(/\s+/g, ' ') }), h('span', { 'class': 'fp', text: L.shortFp(r.fingerprint) + (r.stats && r.stats.flags.length ? ' · ' + r.stats.flags.join(', ') : '') })]); } },
    { key: 'log_calls', label: 'Log calls', num: true, cell: function (r) { return r.log ? L.fmtInt(r.log.calls) : '—'; } },
    { key: 'stats_calls', label: 'Stat calls', num: true, cell: function (r) { return r.stats ? L.fmtInt(r.stats.calls) : '—'; } },
    { key: 'coverage', label: 'Coverage', num: true, cell: function (r) {
      if (r.coverage === null || r.coverage === undefined) { return '—'; }
      var cls = r.coverage < 0.2 ? 'low' : r.coverage > 1.5 ? 'high' : '';
      return h('span', null, [h('span', { 'class': 'gauge ' + cls }, h('b', { style: { width: Math.min(100, r.coverage * 100) + '%' } })), L.fmtCoverage(r.coverage)]);
    } },
    { key: 'total', label: 'Total time', num: true, bar: true, cell: function (r) { return L.fmtMs(L.column(r, 'total')); } },
    { key: 'mean', label: 'Mean', num: true, cell: function (r) { return L.fmtMs(L.column(r, 'mean')); } },
    { key: 'p95', label: 'p95', num: true, cell: function (r) { return L.fmtMs(L.column(r, 'p95')); } },
    { key: 'max', label: 'Max', num: true, cell: function (r) { return L.fmtMs(L.column(r, 'max')); } },
    { key: 'share', label: 'Share', num: true, cell: function (r) { return L.fmtPct(L.column(r, 'share')); } },
    { key: 'errors', label: 'Errors', num: true, cell: function (r) { var e = L.column(r, 'errors'); return e === null ? '—' : L.fmtInt(e); } }
  ];
  var hiddenCols = (function () { try { return JSON.parse(store('pgloupe.cols') || 'null') || ['max', 'errors']; } catch (e) { return ['max', 'errors']; } })();
  var PAGE = 100;

  function viewStatements() {
    var shown = PAGE;
    var cursor = -1;
    var tbody = h('tbody');
    var count = h('span', { 'class': 'count' });
    var more = h('button', { 'class': 'btn more', onclick: function () { shown += PAGE; refresh(); }, text: 'Show more' });
    var head = h('tr');
    var table = h('table', null, [h('thead', null, head), tbody]);
    var search = h('input', { 'class': 'search', type: 'search', placeholder: 'Search statements (press /)', 'aria-label': 'Search statements', value: state.q, id: 'stmt-search' });
    search.value = state.q;
    search.addEventListener('input', function () { state.q = search.value; shown = PAGE; cursor = -1; sync(); refresh(); });
    var chips = h('div', { 'class': 'toolbar' });
    var linkChips = ['all', 'both', 'log', 'stats'].filter(function (l) { return l === 'all' || rows.some(function (r) { return r.link === l; }); });
    var flagSet = {};
    rows.forEach(function (r) { (r.stats ? r.stats.flags : []).forEach(function (f) { flagSet[f] = true; }); });
    var flagChips = Object.keys(flagSet);
    var picker = h('div', { style: { position: 'relative' } });

    function sync() {
      var hash = L.buildHash(state);
      try { history.replaceState(null, '', hash || location.pathname + location.search); } catch (e) { /* the hash is a convenience */ }
    }
    function drawChips() {
      C.clear(chips);
      linkChips.forEach(function (l) {
        chips.appendChild(h('button', { 'class': 'chip', 'aria-pressed': String(state.link === l), onclick: function () { state.link = l; shown = PAGE; sync(); drawChips(); refresh(); }, text: l === 'all' ? 'All' : l }));
      });
      flagChips.forEach(function (f) {
        chips.appendChild(h('button', { 'class': 'chip', 'aria-pressed': String(state.flag === f), onclick: function () { state.flag = state.flag === f ? '' : f; shown = PAGE; sync(); drawChips(); refresh(); }, text: f }));
      });
    }
    function drawHead() {
      C.clear(head);
      COLUMNS.forEach(function (c) {
        if (hiddenCols.indexOf(c.key) !== -1) { return; }
        var active = state.sort === c.key;
        head.appendChild(h('th', {
          'class': (c.num ? 'num ' : '') + 'sortable', 'aria-sort': active ? (state.dir === 'asc' ? 'ascending' : 'descending') : 'none',
          onclick: function () { if (state.sort === c.key) { state.dir = state.dir === 'desc' ? 'asc' : 'desc'; } else { state.sort = c.key; state.dir = c.key === 'statement' || c.key === 'link' ? 'asc' : 'desc'; } sync(); drawHead(); refresh(); }
        }, [c.label, active ? h('span', { 'class': 'arrow', text: state.dir === 'asc' ? '▲' : '▼' }) : null]));
      });
    }
    function visible() {
      var filtered = L.filterStatements(rows, { q: state.q, link: state.link, flag: state.flag });
      return L.sortRows(filtered, function (r) { return L.column(r, state.sort); }, state.dir);
    }
    function refresh() {
      var all = visible();
      var page = all.slice(0, shown);
      var maxTotal = Math.max.apply(null, page.map(function (r) { return L.column(r, 'total') || 0; }).concat([1]));
      C.clear(tbody);
      page.forEach(function (r, index) {
        var tr = h('tr', { 'class': 'clickable' + (r.fingerprint === state.fp ? ' selected' : ''), 'data-fp': r.fingerprint, tabindex: '-1', onclick: function () { cursor = index; openStatement(r.fingerprint); } });
        COLUMNS.forEach(function (c) {
          if (hiddenCols.indexOf(c.key) !== -1) { return; }
          var content = c.cell(r);
          if (c.bar) {
            var value = L.column(r, 'total') || 0;
            content = h('span', null, [h('span', { 'class': 'bar', style: { width: (value / maxTotal * 100).toFixed(1) + '%' } }), h('span', { text: content })]);
            tr.appendChild(h('td', { 'class': 'num bar-cell' }, content));
          } else {
            tr.appendChild(h('td', { 'class': c.num ? 'num' : '' }, content));
          }
        });
        tbody.appendChild(tr);
      });
      count.textContent = all.length === rows.length ? rows.length + ' statements' : all.length + ' of ' + rows.length + ' statements';
      more.hidden = all.length <= shown;
      table.parentNode && (table.parentNode.hidden = all.length === 0);
      empty.hidden = all.length !== 0;
    }
    var empty = emptyNote('No statement matches. Clear the search or the filters.');
    var chooser = h('button', { 'class': 'btn small', 'aria-haspopup': 'true', onclick: function () {
      if (picker.querySelector('.popover')) { picker.querySelector('.popover').remove(); return; }
      var pop = h('div', { 'class': 'popover' }, COLUMNS.map(function (c) {
        var box = h('input', { type: 'checkbox' });
        box.checked = hiddenCols.indexOf(c.key) === -1;
        box.addEventListener('change', function () {
          hiddenCols = hiddenCols.filter(function (k) { return k !== c.key; });
          if (!box.checked) { hiddenCols.push(c.key); }
          store('pgloupe.cols', JSON.stringify(hiddenCols)); drawHead(); refresh();
        });
        return h('label', null, [box, c.label]);
      }));
      picker.appendChild(pop);
    }, text: 'Columns' });
    picker.appendChild(chooser);
    drawChips(); drawHead();
    var wrap = h('div', { 'class': 'table-wrap' }, table);
    setTimeout(refresh, 0);
    viewHooks.statements = {
      move: function (delta) {
        var trs = tbody.querySelectorAll('tr');
        if (!trs.length) { return; }
        cursor = Math.max(0, Math.min(trs.length - 1, cursor + delta));
        trs.forEach(function (t) { t.classList.remove('cursor'); });
        trs[cursor].scrollIntoView({ block: 'nearest' });
        trs[cursor].style.outline = '2px solid var(--accent)';
        trs.forEach(function (t, i) { if (i !== cursor) { t.style.outline = ''; } });
      },
      open: function () { var trs = tbody.querySelectorAll('tr'); if (trs[cursor]) { openStatement(trs[cursor].getAttribute('data-fp')); } },
      focusSearch: function () { search.focus(); search.select(); }
    };
    return [
      h('div', { 'class': 'toolbar' }, [search, picker, count]),
      chips, wrap, empty,
      h('div', { 'class': 'toolbar', style: { 'justify-content': 'center' } }, more)
    ];
  }
  var viewHooks = {};

  function markSelected() {
    document.querySelectorAll('tr[data-fp]').forEach(function (tr) { tr.classList.toggle('selected', tr.getAttribute('data-fp') === state.fp); });
  }

  /* ---------- drawer ---------- */
  var scrim = h('div', { 'class': 'scrim', onclick: function () { setState({ fp: '' }); } });
  var drawerNode = h('aside', { 'class': 'drawer', role: 'dialog', 'aria-label': 'Statement details', 'aria-hidden': 'true' });
  function drawer() {
    var row = state.fp ? byFp[state.fp] : null;
    C.clear(drawerNode);
    if (!row) {
      drawerNode.classList.remove('open'); scrim.classList.remove('open'); drawerNode.setAttribute('aria-hidden', 'true');
      if (drawerCursor) { try { drawerCursor.focus(); } catch (e) { /* the row may be gone */ } drawerCursor = null; }
      return;
    }
    if (!drawerNode.classList.contains('open')) { drawerCursor = document.activeElement; }
    var close = h('button', { 'class': 'btn small close', onclick: function () { setState({ fp: '' }); }, 'aria-label': 'Close', text: 'Close (Esc)' });
    var body = h('div', { 'class': 'body' });
    body.appendChild(h('div', { 'class': 'toolbar' }, [
      h('button', { 'class': 'btn small', onclick: function () { copy(row.text, 'SQL'); }, text: 'Copy SQL' }),
      h('button', { 'class': 'btn small', onclick: function () { copy(row.fingerprint, 'Fingerprint'); }, text: 'Copy fingerprint' })
    ]));
    body.appendChild(sqlNode(row.text));
    var mine = findings.filter(function (f) { return f.statements && f.statements.indexOf(row.fingerprint) !== -1; });
    if (mine.length) { body.appendChild(h('div', { 'class': 'findings' }, mine.map(findingNode))); }
    if (row.log) {
      var lg = row.log;
      body.appendChild(panel('What the log saw', h('div', null, [
        C.ladder(lg),
        facts([
          ['Calls', L.fmtInt(lg.calls)], ['With a duration', L.fmtInt(lg.timed_calls)], ['Total', L.fmtMs(lg.total_ms)], ['Mean', L.fmtMs(lg.mean_ms)],
          ['Min', L.fmtMs(lg.min_ms)], ['Max', L.fmtMs(lg.max_ms)], ['Errors', L.fmtInt(lg.errors)], ['Lock waits', L.fmtInt(lg.lock_waits) + (lg.lock_waits ? ' (' + L.fmtMs(lg.lock_wait_ms) + ')' : '')],
          ['Temp files', L.fmtInt(lg.temp_files) + (lg.temp_files ? ' (' + L.fmtBytes(lg.temp_bytes) + ')' : '')], ['Simple / extended', L.fmtInt(lg.statement_calls) + ' / ' + L.fmtInt(lg.execute_calls)],
          ['First seen', L.fmtTime(lg.first_seen, true)], ['Last seen', L.fmtTime(lg.last_seen, true)]
        ])
      ])));
    }
    if (row.stats) {
      var st = row.stats;
      body.appendChild(panel('What pg_stat_statements saw', facts([
        ['Calls', L.fmtInt(st.calls)], ['Total', L.fmtMs(st.total_exec_ms)], ['Mean', L.fmtMs(st.mean_ms)], ['Share', L.fmtPct(st.share_pct)],
        ['Rows', L.fmtInt(st.rows)], ['Plans', L.fmtInt(st.plans)], ['Shared hit', L.fmtInt(st.shared_blks_hit)], ['Shared read', L.fmtInt(st.shared_blks_read)],
        ['Temp read', L.fmtInt(st.temp_blks_read)], ['Temp written', L.fmtInt(st.temp_blks_written)], ['WAL', L.fmtBytes(st.wal_bytes)], ['Nested calls', L.fmtInt(st.nested_calls)],
        ['Entries', L.fmtInt(st.entries)], ['Query ids', st.queryids.join(', ') + (st.queryid_count > st.queryids.length ? ' (+' + (st.queryid_count - st.queryids.length) + ')' : '')],
        ['Flags', st.flags.length ? st.flags.join(', ') : 'none']
      ])));
    }
    if (row.coverage !== null && row.coverage !== undefined) {
      body.appendChild(panel('Coverage', h('p', { 'class': 'muted', text: 'The log recorded ' + L.fmtCoverage(row.coverage) + ' of the calls pg_stat_statements counted' + (row.query_id_matches === true ? '; the logged query_id matches.' : row.query_id_matches === false ? '; the logged query_id does not match.' : '.') })));
    }
    drawerNode.appendChild(h('div', { 'class': 'drawer-head' }, [badge(row.link, row.link), h('h2', { 'class': 'mono', text: L.shortFp(row.fingerprint) }), close]));
    drawerNode.appendChild(body);
    drawerNode.classList.add('open'); scrim.classList.add('open'); drawerNode.setAttribute('aria-hidden', 'false');
    close.focus();
  }
  function facts(pairs) {
    return h('dl', { 'class': 'facts' }, pairs.map(function (p) { return h('div', null, [h('dt', { text: p[0] }), h('dd', { text: String(p[1]) })]); }));
  }

  /* ---------- other sections ---------- */
  function simpleTable(headers, bodyRows) {
    return h('div', { 'class': 'table-wrap' }, h('table', null, [
      h('thead', null, h('tr', null, headers.map(function (hd) { return h('th', { 'class': hd.num ? 'num' : '', text: hd.label }); }))),
      h('tbody', null, bodyRows)
    ]));
  }
  function viewErrors() {
    var out = [];
    var errs = logs.errors || [];
    var max = Math.max.apply(null, errs.map(function (e) { return e.count; }).concat([1]));
    out.push(errs.length ? simpleTable(
      [{ label: 'Level' }, { label: 'SQLSTATE' }, { label: 'Count', num: true }, { label: 'First message' }, { label: 'First seen' }, { label: 'Last seen' }],
      errs.map(function (e) {
        return h('tr', null, [
          h('td', null, badge(e.level, e.level === 'ERROR' ? 'bad' : 'warn')), h('td', { 'class': 'mono', text: e.sqlstate || '—' }),
          h('td', { 'class': 'num bar-cell' }, [h('span', { 'class': 'bar', style: { width: (e.count / max * 100) + '%' } }), h('span', { text: L.fmtInt(e.count) })]),
          h('td', { text: e.first_message.split('\n')[0] }), h('td', { 'class': 'nowrap', text: L.fmtTime(e.first_seen, true) }), h('td', { 'class': 'nowrap', text: L.fmtTime(e.last_seen, true) })
        ]);
      })) : panel('Errors', emptyNote('No errors in the logs.')));
    var c = logs.cancelled;
    if (c && c.total) {
      var maxReason = Math.max.apply(null, c.by_reason.map(function (r) { return r.count; }));
      var reasons = h('div', { 'class': 'ranked' }, c.by_reason.map(function (r) {
        return h('div', { 'class': 'row' }, [h('span', { 'class': 'name', text: r.reason.replace('_', ' ') }), h('span', { 'class': 'track' }, h('b', { style: { width: (r.count / maxReason * 100) + '%' } })), h('span', { 'class': 'val', text: L.fmtInt(r.count) })]);
      }));
      var list = simpleTable([{ label: 'Statement' }, { label: 'Cancelled', num: true }, { label: 'Calls', num: true }], c.statements.map(function (s) {
        var known = byFp[s.fingerprint];
        return h('tr', { 'class': known ? 'clickable' : '', onclick: known ? function () { openStatement(s.fingerprint); } : null }, [
          h('td', null, h('div', { 'class': 'stmt' }, [h('span', { 'class': 'sql', text: s.text.replace(/\s+/g, ' ') }), h('span', { 'class': 'fp', text: L.shortFp(s.fingerprint) })])),
          h('td', { 'class': 'num', text: L.fmtInt(s.cancelled) }), h('td', { 'class': 'num', text: L.fmtInt(s.calls) })]);
      }));
      out.push(h('div', { 'class': 'grid cols-2' }, [panel('Cancelled by reason', reasons), panel('Cancelled most often', list)]));
    }
    return out;
  }
  function viewLocks() {
    var l = logs.locks, t = logs.temp_files;
    var out = [h('div', { 'class': 'grid kpis' }, [
      kpi('Lock waits', L.fmtInt(l.waits), l.acquired + ' acquired'), kpi('Total wait', L.fmtMs(l.total_wait_ms), 'longest ' + L.fmtMs(l.max_wait_ms)),
      kpi('Deadlocks', L.fmtInt(l.deadlocks), '', null, l.deadlocks ? 'bad' : ''), kpi('Temp files', L.fmtInt(t.files), L.fmtBytes(t.bytes) + ' total'), kpi('Largest temp file', L.fmtBytes(t.max_bytes), '')
    ])];
    var host = h('div', { 'class': 'chart-host' });
    out.push(panel('Lock waits over time', host));
    setTimeout(function () { C.timeline(host, { buckets: display, series: [{ key: 'lock_waits', label: 'Lock waits', color: 'var(--c-locks)' }], range: null }); }, 0);
    var waiters = rows.filter(function (r) { return r.log && r.log.lock_waits > 0; }).sort(function (a, b) { return b.log.lock_wait_ms - a.log.lock_wait_ms; }).slice(0, 15);
    if (waiters.length) {
      out.push(h('h2', { text: 'Statements that waited for locks' }));
      out.push(simpleTable([{ label: 'Statement' }, { label: 'Waits', num: true }, { label: 'Wait time', num: true }], waiters.map(function (r) {
        return h('tr', { 'class': 'clickable', onclick: function () { openStatement(r.fingerprint); } }, [h('td', null, h('div', { 'class': 'stmt' }, h('span', { 'class': 'sql', text: r.text.replace(/\s+/g, ' ') }))), h('td', { 'class': 'num', text: L.fmtInt(r.log.lock_waits) }), h('td', { 'class': 'num', text: L.fmtMs(r.log.lock_wait_ms) })]);
      })));
    }
    var temps = rows.filter(function (r) { return r.log && r.log.temp_files > 0; }).sort(function (a, b) { return b.log.temp_bytes - a.log.temp_bytes; }).slice(0, 15);
    if (temps.length) {
      out.push(h('h2', { text: 'Statements that wrote temp files' }));
      out.push(simpleTable([{ label: 'Statement' }, { label: 'Files', num: true }, { label: 'Size', num: true }], temps.map(function (r) {
        return h('tr', { 'class': 'clickable', onclick: function () { openStatement(r.fingerprint); } }, [h('td', null, h('div', { 'class': 'stmt' }, h('span', { 'class': 'sql', text: r.text.replace(/\s+/g, ' ') }))), h('td', { 'class': 'num', text: L.fmtInt(r.log.temp_files) }), h('td', { 'class': 'num', text: L.fmtBytes(r.log.temp_bytes) })]);
      })));
    }
    return out;
  }
  function viewSessions() {
    var c = logs.connections, q = c.session_ms;
    var host = h('div', { 'class': 'chart-host' });
    setTimeout(function () { C.timeline(host, { buckets: display, series: [{ key: 'connections', label: 'Connections', color: 'var(--c-connections)' }], range: null }); }, 0);
    return [
      h('div', { 'class': 'grid kpis' }, [
        kpi('Received', L.fmtInt(c.received), ''), kpi('Authenticated', L.fmtInt(c.authenticated), ''), kpi('Authorized', L.fmtInt(c.authorized), ''),
        kpi('Disconnections', L.fmtInt(c.disconnections), ''), kpi('Median session', L.fmtMs(q.p50), q.count ? 'p95 ' + L.fmtMs(q.p95) + ', longest ' + L.fmtMs(q.max) : 'no sessions ended')
      ]),
      panel('Connections over time', host),
      histogramPanel('Session durations', logs.histograms.sessions, 'var(--c-connections)')
    ];
  }
  function viewMaintenance() {
    var out = [];
    out.push(logs.checkpoints.length ? simpleTable(
      [{ label: 'Kind' }, { label: 'Started', num: true }, { label: 'Completed', num: true }, { label: 'Reasons' }, { label: 'Buffers', num: true }, { label: 'Write p95', num: true }, { label: 'Sync p95', num: true }, { label: 'Total p95', num: true }],
      logs.checkpoints.map(function (c) {
        return h('tr', null, [h('td', { text: c.kind }), h('td', { 'class': 'num', text: L.fmtInt(c.started) }), h('td', { 'class': 'num', text: L.fmtInt(c.completed) }),
          h('td', null, Object.keys(c.reasons).map(function (k) { return badge(k + ' ×' + c.reasons[k]); })), h('td', { 'class': 'num', text: L.fmtInt(c.buffers) }),
          h('td', { 'class': 'num', text: L.fmtMs(c.write_ms.p95) }), h('td', { 'class': 'num', text: L.fmtMs(c.sync_ms.p95) }), h('td', { 'class': 'num', text: L.fmtMs(c.total_ms.p95) })]);
      })) : panel('Checkpoints', emptyNote('No checkpoints were logged. Set log_checkpoints = on.')));
    var av = logs.autovacuum;
    var maxAv = Math.max.apply(null, av.map(function (t) { return t.total_ms; }).concat([1]));
    out.push(av.length ? simpleTable([{ label: 'Table' }, { label: 'Vacuums', num: true }, { label: 'Analyzes', num: true }, { label: 'Total time', num: true }, { label: 'Longest', num: true }],
      av.map(function (t) {
        return h('tr', null, [h('td', { 'class': 'mono', text: t.table }), h('td', { 'class': 'num', text: L.fmtInt(t.vacuums) }), h('td', { 'class': 'num', text: L.fmtInt(t.analyzes) }),
          h('td', { 'class': 'num bar-cell' }, [h('span', { 'class': 'bar', style: { width: (t.total_ms / maxAv * 100) + '%' } }), h('span', { text: L.fmtMs(t.total_ms) })]), h('td', { 'class': 'num', text: L.fmtMs(t.max_ms) })]);
      })) : panel('Autovacuum', emptyNote('No autovacuum runs were logged (log_autovacuum_min_duration).')));
    return out;
  }
  var breakdownMetric = 'time';
  function viewBreakdowns() {
    var wrap = h('div', { 'class': 'grid cols-2' });
    var chips = h('div', { 'class': 'toolbar' });
    function metricOf(r) { return breakdownMetric === 'time' ? r.total_ms : breakdownMetric === 'errors' ? r.errors : r.statements; }
    function draw() {
      C.clear(chips); C.clear(wrap);
      [['time', 'Statement time'], ['statements', 'Statements'], ['errors', 'Errors']].forEach(function (m) {
        chips.appendChild(h('button', { 'class': 'chip', 'aria-pressed': String(breakdownMetric === m[0]), onclick: function () { breakdownMetric = m[0]; draw(); }, text: m[1] }));
      });
      [['database', 'Databases'], ['user', 'Users'], ['application', 'Applications'], ['host', 'Hosts']].forEach(function (d) {
        var list = (logs.dimensions[d[0]] || []).slice().sort(function (a, b) { return metricOf(b) - metricOf(a); });
        var max = Math.max.apply(null, list.map(metricOf).concat([1]));
        wrap.appendChild(panel(d[1], list.length ? h('div', { 'class': 'ranked' }, list.map(function (r) {
          return h('div', { 'class': 'row', title: r.value }, [h('span', { 'class': 'name', text: r.value }), h('span', { 'class': 'track' }, h('b', { style: { width: (metricOf(r) / max * 100) + '%' } })),
            h('span', { 'class': 'val', text: breakdownMetric === 'time' ? L.fmtMs(r.total_ms) : L.fmtInt(metricOf(r)) + ' · ' + L.fmtInt(r.statements) + ' stmts' })]);
        })) : emptyNote('Nothing recorded.')));
      });
    }
    draw();
    return [chips, wrap];
  }
  function viewSlowest() {
    var s = logs.slowest;
    var out = [];
    if (!s.has_params) { out.push(h('p', { 'class': 'muted', text: 'No row carries bind parameters. They are kept only with --params (when analyzing or ingesting), and only extended-protocol statements log them; they can hold personal data.' })); }
    s.rows.forEach(function (r) {
      var who = [r.database, r.user, r.application, r.host].filter(Boolean).join(' / ');
      var sql = sqlNode(r.text);
      sql.style.maxHeight = '110px';
      out.push(h('div', { 'class': 'panel slow' }, [
        h('div', { 'class': 'head' }, [h('span', { 'class': 'ms', text: L.fmtMs(r.duration_ms) }), h('span', { 'class': 'who', text: L.fmtTime(r.timestamp, true) + (who ? ' · ' + who : '') }),
          byFp[r.fingerprint] ? h('button', { 'class': 'btn small', onclick: function () { openStatement(r.fingerprint); }, text: 'Statement ' + L.shortFp(r.fingerprint) }) : null,
          h('button', { 'class': 'btn small', onclick: function () { sql.style.maxHeight = sql.style.maxHeight ? '' : '110px'; }, text: 'Expand' })]),
        sql, r.params ? h('div', { 'class': 'params', text: 'parameters: ' + r.params }) : null
      ]));
    });
    return out;
  }
  function viewFindings() { return [h('div', { 'class': 'findings' }, findings.map(findingNode))]; }

  var VIEWS = { overview: viewOverview, statements: viewStatements, errors: viewErrors, locks: viewLocks, sessions: viewSessions, maintenance: viewMaintenance, breakdowns: viewBreakdowns, slowest: viewSlowest, findings: viewFindings };

  /* ---------- render ---------- */
  var viewNode = null;
  function render() {
    renderChrome();
    var mainNode = root.querySelector('.main');
    var next = h('div', { 'class': 'view' }, (VIEWS[state.s] || viewOverview)());
    if (viewNode && viewNode.parentNode) { viewNode.parentNode.replaceChild(next, viewNode); } else { mainNode.appendChild(next); }
    viewNode = next;
    drawer();
  }

  document.addEventListener('keydown', function (event) {
    var tag = (event.target && event.target.tagName) || '';
    var typing = tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT';
    if (event.key === 'Escape') {
      if (state.fp) { setState({ fp: '' }); }
      else if (typing) { event.target.blur(); }
      return;
    }
    if (typing || event.ctrlKey || event.metaKey || event.altKey) { return; }
    if (event.key === '/') {
      event.preventDefault();
      if (state.s !== 'statements') { setState({ s: 'statements' }); }
      setTimeout(function () { var s = document.getElementById('stmt-search'); if (s) { s.focus(); s.select(); } }, 0);
    } else if (event.key === 'j' || event.key === 'k') {
      if (state.s === 'statements' && viewHooks.statements) { viewHooks.statements.move(event.key === 'j' ? 1 : -1); }
    } else if (event.key === 'Enter') {
      if (state.s === 'statements' && viewHooks.statements && !state.fp) { viewHooks.statements.open(); }
    } else if (/^[1-9]$/.test(event.key)) {
      var target = sections[Number(event.key) - 1];
      if (target) { setState({ s: target.id, fp: '' }); window.scrollTo(0, 0); }
    }
  });
  var resizeTimer = null;
  window.addEventListener('resize', function () {
    clearTimeout(resizeTimer);
    resizeTimer = setTimeout(function () { if (window.__redraw && state.s === 'overview') { window.__redraw(); } }, 150);
  });
  window.addEventListener('hashchange', function () { state = L.parseHash(location.hash); if (!sections.some(function (s) { return s.id === state.s; })) { state.s = 'overview'; } render(); });

  document.documentElement.classList.add('app');
  applyTheme();
  document.body.appendChild(scrim);
  document.body.appendChild(drawerNode);
  render();
})();
