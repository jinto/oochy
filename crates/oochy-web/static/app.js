'use strict';

// ── State ─────────────────────────────────────────
let selectedAgentId = null;

// ── DOM refs ──────────────────────────────────────
const healthStatus   = document.getElementById('health-status');
const statusDot      = healthStatus.querySelector('.status-dot');
const statusText     = healthStatus.querySelector('.status-text');
const agentListEl    = document.getElementById('agent-list');
const emptyState     = document.getElementById('empty-state');
const convView       = document.getElementById('conversation-view');
const convHeader     = document.getElementById('conv-header');
const messagesEl     = document.getElementById('messages');

// ── Health check ──────────────────────────────────
async function checkHealth() {
  try {
    const res  = await fetch('/api/health');
    const data = await res.json();
    if (data.status === 'ok') {
      statusDot.className  = 'status-dot ok';
      statusText.textContent = `v${data.version}  ●  online`;
    } else {
      setHealthError('degraded');
    }
  } catch (_) {
    setHealthError('offline');
  }
}

function setHealthError(msg) {
  statusDot.className    = 'status-dot err';
  statusText.textContent = msg;
}

// ── Agent list ────────────────────────────────────
async function loadAgents() {
  try {
    const res    = await fetch('/api/agents');
    const agents = await res.json();

    agentListEl.innerHTML = '';

    if (!agents || agents.length === 0) {
      agentListEl.innerHTML = '<li class="agent-item placeholder">No agents yet</li>';
      return;
    }

    for (const agent of agents) {
      const li = document.createElement('li');
      li.className = 'agent-item';
      li.dataset.id = agent.agent_id;

      const idEl  = document.createElement('div');
      idEl.className   = 'agent-id';
      idEl.textContent = agent.agent_id;

      const tsEl  = document.createElement('div');
      tsEl.className   = 'agent-updated';
      tsEl.textContent = formatTimestamp(agent.updated_at);

      li.appendChild(idEl);
      li.appendChild(tsEl);
      li.addEventListener('click', () => selectAgent(agent.agent_id));
      agentListEl.appendChild(li);
    }

    // Re-apply active state if agent still exists
    if (selectedAgentId) {
      const active = agentListEl.querySelector(`[data-id="${CSS.escape(selectedAgentId)}"]`);
      if (active) active.classList.add('active');
    }
  } catch (err) {
    agentListEl.innerHTML = `<li class="agent-item placeholder">Error: ${escHtml(err.message)}</li>`;
  }
}

// ── Select agent ──────────────────────────────────
async function selectAgent(agentId) {
  selectedAgentId = agentId;

  // Update sidebar active state
  agentListEl.querySelectorAll('.agent-item').forEach(el => {
    el.classList.toggle('active', el.dataset.id === agentId);
  });

  // Show conversation view
  emptyState.style.display = 'none';
  convView.style.display   = 'flex';

  convHeader.innerHTML = `<strong>${escHtml(agentId)}</strong>`;
  messagesEl.innerHTML = '<div style="color:var(--text-dim);padding:8px 0">Loading…</div>';

  try {
    const res   = await fetch(`/api/agents/${encodeURIComponent(agentId)}/conversations`);
    const turns = await res.json();
    renderMessages(turns);
  } catch (err) {
    messagesEl.innerHTML = `<div style="color:var(--accent)">Error: ${escHtml(err.message)}</div>`;
  }
}

// ── Render messages ───────────────────────────────
function renderMessages(turns) {
  messagesEl.innerHTML = '';

  if (!turns || turns.length === 0) {
    messagesEl.innerHTML = '<div style="color:var(--text-dim);padding:8px 0">No messages yet.</div>';
    return;
  }

  for (const turn of turns) {
    const role = (turn.role || 'user').toLowerCase();
    const div  = document.createElement('div');
    div.className = `msg ${role}`;

    // Meta line
    const meta   = document.createElement('div');
    meta.className = 'msg-meta';

    const roleEl = document.createElement('span');
    roleEl.className   = 'msg-role';
    roleEl.textContent = role;

    const timeEl = document.createElement('span');
    timeEl.className   = 'msg-time';
    timeEl.textContent = formatTimestamp(turn.timestamp);

    meta.appendChild(roleEl);
    meta.appendChild(timeEl);

    // Content
    const content = document.createElement('div');
    content.className   = 'msg-content';
    content.textContent = turn.content;

    div.appendChild(meta);
    div.appendChild(content);

    // Code block
    if (turn.code) {
      const codeBlock = document.createElement('div');
      codeBlock.className = 'code-block';
      codeBlock.innerHTML =
        '<div class="code-label">CODE</div>' +
        `<pre>${escHtml(turn.code)}</pre>`;
      div.appendChild(codeBlock);
    }

    // Result block
    if (turn.result) {
      const resultBlock = document.createElement('div');
      resultBlock.className = 'result-block';
      resultBlock.innerHTML =
        '<div class="result-label">RESULT</div>' +
        `<pre>${escHtml(turn.result)}</pre>`;
      div.appendChild(resultBlock);
    }

    messagesEl.appendChild(div);
  }

  // Scroll to bottom
  messagesEl.scrollTop = messagesEl.scrollHeight;
}

// ── Helpers ───────────────────────────────────────
function escHtml(str) {
  return String(str)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');
}

function formatTimestamp(ts) {
  if (!ts) return '';
  // Try to parse as ISO or unix timestamp
  const n = Number(ts);
  const d = isNaN(n) ? new Date(ts) : new Date(n * 1000);
  if (isNaN(d.getTime())) return ts;
  return d.toLocaleString(undefined, {
    month: 'short', day: 'numeric',
    hour: '2-digit', minute: '2-digit',
  });
}

// ── Init & polling ────────────────────────────────
async function init() {
  await checkHealth();
  await loadAgents();
}

// Refresh agent list every 10 seconds
setInterval(loadAgents, 10_000);
// Refresh health every 30 seconds
setInterval(checkHealth, 30_000);

init();
