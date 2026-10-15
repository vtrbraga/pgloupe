/* Pure helpers of the dashboard: no DOM access, so `node --test` can run them. */
var PgLib = (function () {
  'use strict';

  var NBSP = ' ';

  function isNum(v) { return typeof v === 'number' && isFinite(v); }

  function fmtInt(n) {
    if (!isNum(n)) { return '—'; }
    var s = String(Math.round(n));
    var out = '';
    for (var i = 0; i < s.length; i++) {
      if (i > 0 && (s.length - i) % 3 === 0 && s[i - 1] !== '-') { out += ','; }
      out += s[i];
    }
    return out;
  }

  /** 12.3 ms, 1.50 s, 2.4 min, 1.2 h. */
  function fmtMs(ms) {
    if (!isNum(ms)) { return '—'; }
    var a = Math.abs(ms);
    if (a >= 3600000) { return (ms / 3600000).toFixed(a >= 36000000 ? 1 : 2) + NBSP + 'h'; }
    if (a >= 60000) { return (ms / 60000).toFixed(a >= 600000 ? 1 : 2) + NBSP + 'min'; }
    if (a >= 1000) { return (ms / 1000).toFixed(a >= 10000 ? 1 : 2) + NBSP + 's'; }
    if (a >= 100) { return ms.toFixed(0) + NBSP + 'ms'; }
    if (a >= 10) { return ms.toFixed(1) + NBSP + 'ms'; }
    if (a >= 1) { return ms.toFixed(2) + NBSP + 'ms'; }
    if (a === 0) { return '0' + NBSP + 'ms'; }
    return ms.toFixed(3) + NBSP + 'ms';
  }

  function fmtBytes(b) {
    if (!isNum(b)) { return '—'; }
    var units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
    var v = b, u = 0;
    while (Math.abs(v) >= 1024 && u < units.length - 1) { v /= 1024; u++; }
    return (u === 0 ? String(Math.round(v)) : v.toFixed(1)) + NBSP + units[u];
  }

  function fmtPct(p, digits) {
    if (!isNum(p)) { return '—'; }
    return p.toFixed(digits === undefined ? 1 : digits) + '%';
  }

  /** A coverage ratio (0.9 = 90%). */
  function fmtCoverage(r) {
    if (!isNum(r)) { return '—'; }
    return Math.round(r * 100) + '%';
  }

  /** 1234567 -> 1.2M, for axis labels and KPI cards. */
  function fmtCompact(n) {
    if (!isNum(n)) { return '—'; }
    var a = Math.abs(n);
    if (a >= 1e9) { return (n / 1e9).toFixed(a >= 1e10 ? 0 : 1) + 'B'; }
    if (a >= 1e6) { return (n / 1e6).toFixed(a >= 1e7 ? 0 : 1) + 'M'; }
    if (a >= 1e3) { return (n / 1e3).toFixed(a >= 1e4 ? 0 : 1) + 'k'; }
    return String(Math.round(n));
  }

  function shortFp(fp) { return typeof fp === 'string' ? fp.slice(0, 8) : ''; }

  /** `2026-10-14T10:20:09Z` -> `10:20:09`, or the date too when asked. */
  function fmtTime(ts, withDate) {
    if (typeof ts !== 'string') { return '—'; }
    var m = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})/.exec(ts);
    if (!m) { return ts; }
    return withDate ? m[1] + ' ' + m[2] : m[2];
  }

  /** Compares two possibly missing values; missing values sort last in either direction. */
  function compareValues(a, b, dir) {
    var am = a === null || a === undefined || (typeof a === 'number' && !isFinite(a));
    var bm = b === null || b === undefined || (typeof b === 'number' && !isFinite(b));
    if (am && bm) { return 0; }
    if (am) { return 1; }
    if (bm) { return -1; }
    var c;
    if (typeof a === 'string' || typeof b === 'string') {
      c = String(a).localeCompare(String(b));
    } else {
      c = a < b ? -1 : a > b ? 1 : 0;
    }
    return dir === 'desc' ? -c : c;
  }

  /** A sorted copy; ties keep their original order. */
  function sortRows(rows, accessor, dir) {
    return rows
      .map(function (row, index) { return { row: row, index: index, key: accessor(row) }; })
      .sort(function (x, y) { return compareValues(x.key, y.key, dir) || x.index - y.index; })
      .map(function (x) { return x.row; });
  }

  /** Rows matching the search words (all of them) and the chosen link and flag. */
  function filterStatements(rows, query) {
    var words = String(query.q || '').toLowerCase().split(/\s+/).filter(Boolean);
    return rows.filter(function (row) {
      if (query.link && query.link !== 'all' && row.link !== query.link) { return false; }
      if (query.flag) {
        var flags = (row.stats && row.stats.flags) || [];
        if (flags.indexOf(query.flag) === -1) { return false; }
      }
      if (words.length) {
        var hay = (row.text + ' ' + row.fingerprint).toLowerCase();
        for (var i = 0; i < words.length; i++) {
          if (hay.indexOf(words[i]) === -1) { return false; }
        }
      }
      return true;
    });
  }

  /** The value of a column for a joined statement row. */
  function column(row, key) {
    var log = row.log, stats = row.stats;
    switch (key) {
      case 'link': return row.link;
      case 'log_calls': return log ? log.calls : null;
      case 'stats_calls': return stats ? stats.calls : null;
      case 'coverage': return row.coverage;
      case 'total': return stats ? stats.total_exec_ms : (log ? log.total_ms : null);
      case 'mean': return stats ? stats.mean_ms : (log ? log.mean_ms : null);
      case 'p95': return log ? log.p95_ms : null;
      case 'max': return log ? log.max_ms : null;
      case 'share': return stats ? stats.share_pct : (log ? log.share_pct : null);
      case 'errors': return log ? log.errors : null;
      case 'statement': return row.text;
      default: return null;
    }
  }

  /** Merges consecutive buckets so that at most `max` remain; counts are summed. */
  function downsample(buckets, max) {
    if (buckets.length <= max) {
      return buckets.map(function (b) { return Object.assign({ span: 1 }, b); });
    }
    var group = Math.ceil(buckets.length / max);
    var out = [];
    for (var i = 0; i < buckets.length; i += group) {
      var chunk = buckets.slice(i, i + group);
      var merged = { start: chunk[0].start, span: chunk.length, statements: 0, errors: 0, connections: 0, lock_waits: 0 };
      chunk.forEach(function (b) {
        merged.statements += b.statements;
        merged.errors += b.errors;
        merged.connections += b.connections;
        merged.lock_waits += b.lock_waits;
      });
      out.push(merged);
    }
    return out;
  }

  /** Sums one series over the inclusive bucket range [from, to]; the whole series without a range. */
  function sumRange(buckets, key, range) {
    var from = range ? Math.max(0, Math.min(range.from, range.to)) : 0;
    var to = range ? Math.min(buckets.length - 1, Math.max(range.from, range.to)) : buckets.length - 1;
    var total = 0;
    for (var i = from; i <= to; i++) { total += buckets[i][key] || 0; }
    return total;
  }

  /** A round number at or above `value`, for the top of an axis. */
  function niceMax(value) {
    if (!(value > 0)) { return 1; }
    var exp = Math.pow(10, Math.floor(Math.log10(value)));
    var f = value / exp;
    var nice = f <= 1 ? 1 : f <= 2 ? 2 : f <= 5 ? 5 : 10;
    return nice * exp;
  }

  var KEYWORDS = new Set(('select insert update delete from where join left right inner outer full cross on and or not in is null ' +
    'as order by group having limit offset union all distinct values set into returning with recursive case when then else end ' +
    'create alter drop table index view begin commit rollback savepoint copy truncate exists between like ilike any some using ' +
    'asc desc over partition window filter lateral conflict do nothing default primary key references foreign unique check ' +
    'explain analyze vacuum grant revoke call true false').split(' '));

  /** Splits SQL into `{t, v}` tokens (kw, str, num, param, comment, id, op, ws) that join back to the input. */
  function tokenizeSql(sql) {
    var out = [];
    var i = 0;
    var n = sql.length;
    function push(t, v) { out.push({ t: t, v: v }); }
    while (i < n) {
      var c = sql[i];
      var two = sql.substr(i, 2);
      if (/\s/.test(c)) {
        var j = i + 1;
        while (j < n && /\s/.test(sql[j])) { j++; }
        push('ws', sql.slice(i, j)); i = j;
      } else if (two === '--') {
        var e = sql.indexOf('\n', i);
        if (e === -1) { e = n; }
        push('comment', sql.slice(i, e)); i = e;
      } else if (two === '/*') {
        var end = sql.indexOf('*/', i + 2);
        end = end === -1 ? n : end + 2;
        push('comment', sql.slice(i, end)); i = end;
      } else if (c === '\'') {
        var k = i + 1;
        while (k < n) {
          if (sql[k] === '\'') {
            if (sql[k + 1] === '\'') { k += 2; continue; }
            k++; break;
          }
          k++;
        }
        push('str', sql.slice(i, k)); i = k;
      } else if (c === '"') {
        var q = sql.indexOf('"', i + 1);
        q = q === -1 ? n : q + 1;
        push('id', sql.slice(i, q)); i = q;
      } else if (c === '$' && /\d/.test(sql[i + 1] || '')) {
        var p = i + 1;
        while (p < n && /\d/.test(sql[p])) { p++; }
        push('param', sql.slice(i, p)); i = p;
      } else if (/\d/.test(c)) {
        var d = i + 1;
        while (d < n && /[\d.eE]/.test(sql[d])) { d++; }
        push('num', sql.slice(i, d)); i = d;
      } else if (/[A-Za-z_]/.test(c)) {
        var w = i + 1;
        while (w < n && /[A-Za-z0-9_$]/.test(sql[w])) { w++; }
        var word = sql.slice(i, w);
        push(KEYWORDS.has(word.toLowerCase()) ? 'kw' : 'id', word); i = w;
      } else {
        push('op', c); i++;
      }
    }
    return out;
  }

  var DEFAULT_STATE = { s: 'overview', q: '', sort: 'total', dir: 'desc', link: 'all', flag: '', fp: '', from: null, to: null, th: '' };

  /** `#s=statements&q=film` -> state object (unknown keys ignored, bad numbers dropped). */
  function parseHash(hash) {
    var state = Object.assign({}, DEFAULT_STATE);
    var text = String(hash || '').replace(/^#/, '');
    if (!text) { return state; }
    text.split('&').forEach(function (pair) {
      var eq = pair.indexOf('=');
      if (eq < 1) { return; }
      var key = pair.slice(0, eq);
      var value;
      try { value = decodeURIComponent(pair.slice(eq + 1).replace(/\+/g, ' ')); } catch (e) { return; }
      if (!Object.prototype.hasOwnProperty.call(DEFAULT_STATE, key)) { return; }
      if (key === 'from' || key === 'to') {
        var n = parseInt(value, 10);
        state[key] = isFinite(n) && n >= 0 ? n : null;
      } else if (key === 'dir') {
        state.dir = value === 'asc' ? 'asc' : 'desc';
      } else {
        state[key] = value;
      }
    });
    return state;
  }

  /** Only the parts that differ from the defaults, so URLs stay short. */
  function buildHash(state) {
    var parts = [];
    Object.keys(DEFAULT_STATE).forEach(function (key) {
      var v = state[key];
      if (v === null || v === undefined || v === DEFAULT_STATE[key]) { return; }
      parts.push(key + '=' + encodeURIComponent(String(v)));
    });
    return parts.length ? '#' + parts.join('&') : '';
  }

  /** Where a value sits on a log scale between `lo` and `hi`, 0..1. */
  function logPosition(value, lo, hi) {
    if (!(value > 0) || !(lo > 0) || !(hi > lo)) { return 0; }
    var p = (Math.log(value) - Math.log(lo)) / (Math.log(hi) - Math.log(lo));
    return Math.max(0, Math.min(1, p));
  }

  /** A selection of `[from, to]` over `length` buckets as an ordered, in-bounds pair; `null` if none of it is in bounds. */
  function clampRange(from, to, length) {
    if (!isNum(from) || !isNum(to) || !(length > 0)) { return null; }
    var a = Math.min(from, to), b = Math.max(from, to), last = length - 1;
    if (a > last || b < 0) { return null; }
    return { from: Math.max(0, a), to: Math.min(b, last) };
  }

  var api = {
    clampRange: clampRange,
    fmtInt: fmtInt, fmtMs: fmtMs, fmtBytes: fmtBytes, fmtPct: fmtPct, fmtCoverage: fmtCoverage, fmtCompact: fmtCompact,
    shortFp: shortFp, fmtTime: fmtTime, compareValues: compareValues, sortRows: sortRows,
    filterStatements: filterStatements, column: column, downsample: downsample, sumRange: sumRange,
    niceMax: niceMax, tokenizeSql: tokenizeSql, parseHash: parseHash, buildHash: buildHash,
    logPosition: logPosition, DEFAULT_STATE: DEFAULT_STATE
  };
  if (typeof module !== 'undefined' && module.exports) { module.exports = api; }
  return api;
})();
