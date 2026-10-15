/* Live mode: polls the watch server and swaps newer reports into the dashboard. Does nothing without its config. */
(function () {
  'use strict';
  var node = document.getElementById('pgloupe-live');
  if (!node || !window.PgApp) { return; }
  var config;
  try { config = JSON.parse(node.textContent); } catch (e) { return; }
  var L = PgLib;
  var pollMs = Math.max(5, Number(config.poll_secs) || 15) * 1000;
  // The page is a report the server already built: do not fetch it again on the first poll.
  var loaded = isFinite(Number(config.generation)) ? { generation: Number(config.generation), run_id: config.run_id } : null;
  var status = null, paused = false, unreachable = false, busy = false, display = '';

  var text = document.createElement('span');
  var problem = document.createElement('span');
  problem.className = 'live-problem';
  var toggle = document.createElement('button');
  toggle.type = 'button';
  toggle.textContent = 'Pause';
  var pill = document.createElement('div');
  pill.className = 'live-pill';
  pill.setAttribute('role', 'status');
  [text, problem, toggle].forEach(function (n) { pill.appendChild(n); });
  document.body.appendChild(pill);

  function seconds(iso) {
    var t = Date.parse(iso || '');
    return isNaN(t) ? NaN : (Date.now() - t) / 1000;
  }

  function paint() {
    var updated = status && status.updated_at ? L.fmtAgo(seconds(status.updated_at)) : 'waiting for the first report';
    var next = status && status.next_at ? L.fmtIn(-seconds(status.next_at)) : null;
    pill.classList.toggle('paused', paused);
    pill.classList.toggle('failing', !!display || unreachable || !!(status && status.last_error));
    text.textContent = (paused ? '❚❚ paused' : '● live') + ' · updated ' + updated + (next && !paused ? ' · next ' + next : '');
    var msg = '';
    if (display) { msg = display + ', showing data from ' + updated; }
    else if (unreachable) { msg = 'server unreachable, showing data from ' + updated; }
    else if (status && status.last_error) { msg = 'last update failed (' + status.last_error + '), showing data from ' + updated; }
    else if (status && status.notes && status.notes.length) { msg = 'note: ' + status.notes.join('; '); }
    problem.textContent = msg;
    problem.hidden = !msg;
  }

  function load(url, then) {
    return fetch(url, { cache: 'no-store' }).then(function (r) {
      if (!r.ok) { throw new Error(String(r.status)); }
      return then(r);
    });
  }

  function poll() {
    if (paused || busy || document.hidden) { return; }
    busy = true;
    load('/status', function (r) { return r.json(); }).then(function (s) {
      status = s;
      unreachable = false;
      if (!L.shouldFetch(s, loaded)) { return null; }
      return load('/report.json', function (r) { return r.text(); }).then(function (raw) {
        // A report the page cannot show is not a server that went away.
        try {
          display = window.PgApp.update(raw) ? '' : 'could not display the update';
        } catch (e) {
          display = 'could not display the update';
        }
        if (!display) { loaded = { generation: s.generation, run_id: s.run_id }; }
      });
    }).catch(function () {
      unreachable = true;
    }).then(function () {
      busy = false;
      paint();
    });
  }

  toggle.addEventListener('click', function () {
    paused = !paused;
    toggle.textContent = paused ? 'Resume' : 'Pause';
    paint();
    poll();
  });
  document.addEventListener('visibilitychange', function () { poll(); });
  setInterval(poll, pollMs);
  setInterval(paint, 1000);
  paint();
  poll();
})();
