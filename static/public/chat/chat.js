(() => {
  const modelSelect = document.getElementById('modelSelect');
  const reasoningSelect = document.getElementById('reasoningSelect');
  const tempRange = document.getElementById('tempRange');
  const tempValue = document.getElementById('tempValue');
  const topPRange = document.getElementById('topPRange');
  const topPValue = document.getElementById('topPValue');
  const systemInput = document.getElementById('systemInput');
  const promptInput = document.getElementById('promptInput');
  const sendBtn = document.getElementById('sendBtn');
  const settingsToggle = document.getElementById('settingsToggle');
  const settingsPanel = document.getElementById('settingsPanel');
  const chatLog = document.getElementById('chatLog');
  const emptyState = document.getElementById('emptyState');
  const statusText = document.getElementById('statusText');
  const attachBtn = document.getElementById('attachBtn');
  const fileInput = document.getElementById('fileInput');
  const fileBadge = document.getElementById('fileBadge');
  const fileName = document.getElementById('fileName');
  const fileRemoveBtn = document.getElementById('fileRemoveBtn');

  let messageHistory = [];
  let isSending = false;
  let abortController = null;
  let attachment = null;
  const feedbackUrl = 'https://github.com/XeanYu/grok2api-rs/issues/new';

  function toast(message, type) {
    if (typeof showToast === 'function') showToast(message, type);
  }

  function setStatus(state, text) {
    if (!statusText) return;
    statusText.textContent = text || '就绪';
    statusText.classList.remove('connected', 'connecting', 'error');
    if (state) statusText.classList.add(state);
  }

  function setSendingState(sending) {
    isSending = sending;
    if (sendBtn) sendBtn.disabled = sending;
  }

  function updateRangeValues() {
    if (tempValue && tempRange) tempValue.textContent = Number(tempRange.value).toFixed(2);
    if (topPValue && topPRange) topPValue.textContent = Number(topPRange.value).toFixed(2);
  }

  function scrollToBottom() {
    const body = document.scrollingElement || document.documentElement;
    if (!body) return;
    const hasOwnScroll = chatLog && chatLog.scrollHeight > chatLog.clientHeight + 1;
    if (hasOwnScroll) { chatLog.scrollTop = chatLog.scrollHeight; return; }
    body.scrollTop = body.scrollHeight;
  }

  function hideEmptyState() { if (emptyState) emptyState.classList.add('hidden'); }
  function showEmptyState() { if (emptyState) emptyState.classList.remove('hidden'); }

  function escapeHtml(value) {
    return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }

  function renderBasicMarkdown(rawText) {
    const text = (rawText || '').replace(/\\n/g, '\n');
    const escaped = escapeHtml(text);
    const codeBlocks = [];
    const fenced = escaped.replace(/```([a-zA-Z0-9_-]+)?\n([\s\S]*?)```/g, (match, lang, code) => {
      const safeLang = lang ? escapeHtml(lang) : '';
      const html = `<pre class="code-block"><code${safeLang ? ` class="language-${safeLang}"` : ''}>${code}</code></pre>`;
      const token = `@@CODEBLOCK_${codeBlocks.length}@@`;
      codeBlocks.push(html);
      return token;
    });

    const renderInline = (value) => {
      let output = value
        .replace(/`([^`]+)`/g, '<code class="inline-code">$1</code>')
        .replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>')
        .replace(/\*([^*]+)\*/g, '<em>$1</em>');
      output = output.replace(/!\[([^\]]*)\]\(([^)]+)\)/g, (m, alt, url) => {
        return `<img src="${escapeHtml(url||'')}" alt="${escapeHtml(alt||'image')}" loading="lazy">`;
      });
      output = output.replace(/\[([^\]]+)\]\(([^)]+)\)/g, (m, label, url) => {
        return `<a href="${escapeHtml(url||'')}" target="_blank" rel="noopener">${escapeHtml(label||'')}</a>`;
      });
      output = output.replace(/(data:image\/[a-zA-Z0-9.+-]+;base64,[A-Za-z0-9+/=]+)/g, (m, uri) => {
        return `<img src="${escapeHtml(uri||'')}" alt="image" loading="lazy">`;
      });
      return output;
    };

    const lines = fenced.split(/\r?\n/);
    const htmlParts = [];
    let inUl = false, inOl = false, inTable = false, paragraphLines = [];

    const closeLists = () => { if (inUl) { htmlParts.push('</ul>'); inUl = false; } if (inOl) { htmlParts.push('</ol>'); inOl = false; } };
    const closeTable = () => { if (inTable) { htmlParts.push('</tbody></table>'); inTable = false; } };
    const flushParagraph = () => { if (!paragraphLines.length) return; htmlParts.push(`<p>${renderInline(paragraphLines.join('<br>'))}</p>`); paragraphLines = []; };
    const isTableSeparator = (line) => /^\s*\|?(?:\s*:?-+:?\s*\|)+\s*$/.test(line);
    const splitTableRow = (line) => { const t = line.trim().replace(/^\|/, '').replace(/\|$/, ''); return t.split('|').map(c => c.trim()); };

    for (let i = 0; i < lines.length; i++) {
      const line = lines[i], trimmed = line.trim();
      if (!trimmed) { flushParagraph(); closeLists(); closeTable(); continue; }
      if (/^@@CODEBLOCK_\d+@@$/.test(trimmed)) { flushParagraph(); closeLists(); closeTable(); htmlParts.push(trimmed); continue; }
      const hm = trimmed.match(/^(#{1,6})\s+(.*)$/);
      if (hm) { flushParagraph(); closeLists(); closeTable(); htmlParts.push(`<h${hm[1].length}>${renderInline(hm[2])}</h${hm[1].length}>`); continue; }
      if (trimmed.includes('|')) {
        const next = lines[i + 1] || '';
        if (!inTable && isTableSeparator(next.trim())) {
          flushParagraph(); closeLists();
          const headers = splitTableRow(trimmed);
          htmlParts.push('<div class="table-wrap"><table><thead><tr>');
          headers.forEach(c => htmlParts.push(`<th>${renderInline(c)}</th>`));
          htmlParts.push('</tr></thead><tbody>');
          inTable = true; i++; continue;
        }
        if (inTable && !isTableSeparator(trimmed)) {
          const cells = splitTableRow(trimmed);
          htmlParts.push('<tr>'); cells.forEach(c => htmlParts.push(`<td>${renderInline(c)}</td>`)); htmlParts.push('</tr>'); continue;
        }
      }
      const ulm = trimmed.match(/^[-*+•·]\s+(.*)$/);
      if (ulm) { flushParagraph(); if (!inUl) { closeLists(); closeTable(); htmlParts.push('<ul>'); inUl = true; } htmlParts.push(`<li>${renderInline(ulm[1])}</li>`); continue; }
      const olm = trimmed.match(/^\d+[.)、]\s+(.*)$/);
      if (olm) { flushParagraph(); if (!inOl) { closeLists(); closeTable(); htmlParts.push('<ol>'); inOl = true; } htmlParts.push(`<li>${renderInline(olm[1])}</li>`); continue; }
      paragraphLines.push(trimmed);
    }
    flushParagraph(); closeLists(); closeTable();
    let output = htmlParts.join('');
    codeBlocks.forEach((html, idx) => { output = output.replace(`@@CODEBLOCK_${idx}@@`, html); });
    return output;
  }

  function parseThinkSections(raw) {
    const parts = []; let cursor = 0;
    while (cursor < raw.length) {
      const start = raw.indexOf('<think>', cursor);
      if (start === -1) { parts.push({ type: 'text', value: raw.slice(cursor) }); break; }
      if (start > cursor) parts.push({ type: 'text', value: raw.slice(cursor, start) });
      const ts = start + 7, end = raw.indexOf('</think>', ts);
      if (end === -1) { parts.push({ type: 'think', value: raw.slice(ts), open: true }); cursor = raw.length; }
      else { parts.push({ type: 'think', value: raw.slice(ts, end), open: false }); cursor = end + 8; }
    }
    return parts;
  }

  function parseRolloutBlocks(text) {
    const lines = (text || '').split(/\r?\n/), blocks = []; let current = null;
    for (const line of lines) {
      const m = line.match(/^\s*\[([^\]]+)\]\[([^\]]+)\]\s*(.*)$/);
      if (m) { if (current) blocks.push(current); current = { id: m[1], type: m[2], lines: [] }; if (m[3]) current.lines.push(m[3]); continue; }
      if (current) current.lines.push(line);
    }
    if (current) blocks.push(current);
    return blocks;
  }

  function parseAgentSections(text) {
    const lines = (text || '').split(/\r?\n/), sections = [];
    let current = { title: null, lines: [] }, hasAgent = false;
    for (const line of lines) {
      const trimmed = line.trim();
      if (!trimmed) { current.lines.push(line); continue; }
      const am = trimmed.match(/^(Grok\s+Leader|Agent\s*\d+|Grok\s+Agent\s*\d+)$/i);
      if (am) { hasAgent = true; if (current.lines.length) sections.push(current); current = { title: am[1], lines: [] }; continue; }
      current.lines.push(line);
    }
    if (current.lines.length) sections.push(current);
    return hasAgent ? sections : [{ title: null, lines }];
  }

  function renderThinkContent(text, openAll) {
    const sections = parseAgentSections(text);
    if (!sections.length) return renderBasicMarkdown(text);
    const renderGroups = (blocks, openAllGroups) => {
      const groups = [], map = new Map();
      for (const b of blocks) { let g = map.get(b.id); if (!g) { g = { id: b.id, items: [] }; map.set(b.id, g); groups.push(g); } g.items.push(b); }
      return groups.map(g => {
        const items = g.items.map(item => {
          const body = renderBasicMarkdown(item.lines.join('\n').trim());
          const tk = String(item.type||'').trim().toLowerCase().replace(/\s+/g,'');
          return `<div class="think-item-row"><div class="think-item-type" data-type="${escapeHtml(tk)}">${escapeHtml(item.type)}</div><div class="think-item-body">${body||'<em>（空）</em>'}</div></div>`;
        }).join('');
        const oa = openAllGroups ? ' open' : '';
        return `<details class="think-rollout-group"${oa}><summary><span class="think-rollout-title">${escapeHtml(g.id)}</span></summary><div class="think-rollout-body">${items}</div></details>`;
      }).join('');
    };
    const agentBlocks = sections.map((s, idx) => {
      const blocks = parseRolloutBlocks(s.lines.join('\n'));
      const inner = blocks.length ? renderGroups(blocks, openAll) : `<div class="think-rollout-body">${renderBasicMarkdown(s.lines.join('\n').trim())}</div>`;
      if (!s.title) return `<div class="think-agent-items">${inner}</div>`;
      const oa = openAll ? ' open' : (idx === 0 ? ' open' : '');
      return `<details class="think-agent"${oa}><summary>${escapeHtml(s.title)}</summary><div class="think-agent-items">${inner}</div></details>`;
    });
    return `<div class="think-agents">${agentBlocks.join('')}</div>`;
  }

  function renderMarkdown(text) {
    const parts = parseThinkSections(text || '');
    return parts.map(p => {
      if (p.type === 'think') {
        const body = renderThinkContent(p.value.trim(), p.open);
        return `<details class="think-block" data-think="true"${p.open?' open':''}><summary class="think-summary">思考</summary><div class="think-content">${body||'<em>（空）</em>'}</div></details>`;
      }
      return renderBasicMarkdown(p.value);
    }).join('');
  }

  function createMessage(role, content) {
    if (!chatLog) return null;
    hideEmptyState();
    const row = document.createElement('div');
    row.className = `message-row ${role === 'user' ? 'user' : 'assistant'}`;
    const bubble = document.createElement('div');
    bubble.className = 'message-bubble';
    const contentNode = document.createElement('div');
    contentNode.className = 'message-content';
    contentNode.textContent = content || '';
    bubble.appendChild(contentNode);
    row.appendChild(bubble);
    chatLog.appendChild(row);
    scrollToBottom();
    return { row, contentNode, role, raw: content||'', committed: false, startedAt: Date.now(), firstTokenAt: null, hasThink: false, thinkElapsed: null };
  }

  function applyImageGrid(root) {
    if (!root) return;
    const isIgnorable = n => (n.nodeType === Node.TEXT_NODE && !n.textContent.trim()) || (n.nodeType === Node.ELEMENT_NODE && n.tagName === 'BR');
    const isImageLink = n => n && n.nodeType === Node.ELEMENT_NODE && n.tagName === 'A' && Array.from(n.childNodes).length && Array.from(n.childNodes).every(c => (c.nodeType === Node.TEXT_NODE && !c.textContent.trim()) || (c.nodeType === Node.ELEMENT_NODE && c.tagName === 'IMG'));
    const extractImageItems = n => {
      if (!n || n.nodeType !== Node.ELEMENT_NODE) return null;
      if (n.classList && n.classList.contains('img-grid')) return null;
      if (n.tagName === 'IMG') return { items: [n], removeNode: null };
      if (isImageLink(n)) return { items: [n], removeNode: null };
      if (n.tagName === 'P') {
        const items = [], children = Array.from(n.childNodes);
        if (!children.length) return null;
        for (const c of children) {
          if (c.nodeType === Node.TEXT_NODE) { if (!c.textContent.trim()) continue; return null; }
          if (c.nodeType === Node.ELEMENT_NODE) { if (c.tagName === 'IMG' || isImageLink(c)) { items.push(c); continue; } if (c.tagName === 'BR') continue; return null; }
          return null;
        }
        return items.length ? { items, removeNode: n } : null;
      }
      return null;
    };
    const wrap = container => {
      const children = Array.from(container.childNodes); let group = [], groupStart = null, removeNodes = [];
      const flush = () => {
        if (group.length < 2) { group = []; groupStart = null; removeNodes = []; return; }
        const w = document.createElement('div'); w.className = 'img-grid'; w.style.setProperty('--cols', String(Math.min(4, group.length)));
        if (groupStart) container.insertBefore(w, groupStart); else container.appendChild(w);
        group.forEach(img => w.appendChild(img));
        removeNodes.forEach(n => n.parentNode && n.parentNode.removeChild(n));
        group = []; groupStart = null; removeNodes = [];
      };
      children.forEach(node => {
        if (group.length && isIgnorable(node)) { removeNodes.push(node); return; }
        const ex = extractImageItems(node);
        if (ex && ex.items.length) { if (!groupStart) groupStart = node; group.push(...ex.items); if (ex.removeNode) removeNodes.push(ex.removeNode); return; }
        flush();
      });
      flush();
    };
    [root, ...root.querySelectorAll('.think-content, .think-item-body, .think-rollout-body, .think-agent-items')].forEach(c => {
      if (!c || c.closest('.img-grid') || !c.querySelector || !c.querySelector('img')) return;
      wrap(c);
    });
  }

  function updateMessage(entry, content, finalize = false) {
    if (!entry) return;
    entry.raw = content || '';
    if (!entry.contentNode) return;
    if (!entry.hasThink && entry.raw.includes('<think>')) entry.hasThink = true;
    if (finalize) { entry.contentNode.classList.add('rendered'); entry.contentNode.innerHTML = renderMarkdown(entry.raw); }
    else { entry.contentNode.innerHTML = entry.role === 'assistant' ? renderMarkdown(entry.raw) : entry.raw; }
    if (entry.hasThink) updateThinkSummary(entry, entry.thinkElapsed);
    if (entry.role === 'assistant') {
      applyImageGrid(entry.contentNode);
      entry.contentNode.querySelectorAll('.think-content').forEach(n => { n.scrollTop = n.scrollHeight; });
      enhanceBrokenImages(entry.contentNode);
      if (finalize && entry.row && !entry.row.querySelector('.message-actions')) attachAssistantActions(entry);
    }
    scrollToBottom();
  }

  function enhanceBrokenImages(root) {
    if (!root) return;
    root.querySelectorAll('img').forEach(img => {
      if (img.dataset.retryBound) return;
      img.dataset.retryBound = '1';
      img.addEventListener('error', () => {
        if (img.dataset.failed) return;
        img.dataset.failed = '1';
        const btn = document.createElement('button'); btn.type = 'button'; btn.className = 'img-retry'; btn.textContent = '点击重试';
        btn.addEventListener('click', () => { const src = img.getAttribute('src')||''; img.dataset.failed = ''; img.src = `${src}${src.includes('?')?'&':'?'}t=${Date.now()}`; });
        img.replaceWith(btn);
      });
      img.addEventListener('load', () => { if (img.dataset.failed) img.dataset.failed = ''; });
    });
  }

  function updateThinkSummary(entry, elapsedSec) {
    if (!entry || !entry.contentNode) return;
    const text = typeof elapsedSec === 'number' ? `思考 ${elapsedSec} 秒` : '思考中';
    entry.contentNode.querySelectorAll('.think-summary').forEach(node => {
      node.textContent = text;
      const block = node.closest('.think-block');
      if (!block) return;
      if (typeof elapsedSec === 'number') block.removeAttribute('data-thinking');
      else block.setAttribute('data-thinking', 'true');
    });
  }

  function buildMessages() { return buildMessagesFrom(messageHistory); }
  function buildMessagesFrom(history) {
    const payload = [];
    const sys = systemInput ? systemInput.value.trim() : '';
    if (sys) payload.push({ role: 'system', content: sys });
    for (const msg of history) payload.push({ role: msg.role, content: msg.content });
    return payload;
  }

  function buildPayload() {
    const p = { model: (modelSelect&&modelSelect.value)||'grok-3', messages: buildMessages(), stream: true, temperature: Number(tempRange?tempRange.value:0.8), top_p: Number(topPRange?topPRange.value:0.95) };
    const r = reasoningSelect ? reasoningSelect.value : '';
    if (r) p.reasoning_effort = r;
    return p;
  }
  function buildPayloadFrom(history) {
    const p = { model: (modelSelect&&modelSelect.value)||'grok-3', messages: buildMessagesFrom(history), stream: true, temperature: Number(tempRange?tempRange.value:0.8), top_p: Number(topPRange?topPRange.value:0.95) };
    const r = reasoningSelect ? reasoningSelect.value : '';
    if (r) p.reasoning_effort = r;
    return p;
  }

  async function loadModels() {
    if (!modelSelect) return;
    modelSelect.innerHTML = '';
    const fallback = ['grok-4.1-fast','grok-4','grok-3','grok-3-mini','grok-3-thinking','grok-4.20-beta'];
    const preferred = 'grok-4.20-beta';
    try {
      const res = await fetch('/v1/models', { cache: 'no-store' });
      if (!res.ok) throw new Error('fail');
      const data = await res.json();
      const items = Array.isArray(data&&data.data) ? data.data : [];
      const ids = items.map(i => i&&i.id).filter(Boolean).filter(id => !String(id).startsWith('grok-imagine')).filter(id => !String(id).includes('video'));
      const list = ids.length ? ids : fallback;
      list.forEach(id => { const o = document.createElement('option'); o.value = id; o.textContent = id; modelSelect.appendChild(o); });
      modelSelect.value = list.includes(preferred) ? preferred : (list[list.length-1]||preferred);
    } catch (e) {
      fallback.forEach(id => { const o = document.createElement('option'); o.value = id; o.textContent = id; modelSelect.appendChild(o); });
      modelSelect.value = preferred;
    }
  }

  function showAttachmentBadge() {
    if (!fileBadge || !fileName) return;
    if (attachment) { fileName.textContent = attachment.name; fileBadge.classList.remove('hidden'); }
    else { fileBadge.classList.add('hidden'); fileName.textContent = ''; }
  }
  function clearAttachment() { attachment = null; if (fileInput) fileInput.value = ''; showAttachmentBadge(); }
  function readFileAsDataUrl(file) { return new Promise((ok, fail) => { const r = new FileReader(); r.onload = () => ok(r.result); r.onerror = () => fail(new Error('文件读取失败')); r.readAsDataURL(file); }); }
  async function handleFileSelect(file) {
    if (!file) return;
    try { attachment = { name: file.name||'file', data: await readFileAsDataUrl(file) }; showAttachmentBadge(); }
    catch (e) { toast('文件读取失败', 'error'); }
  }

  function createActionButton(label, title, onClick) {
    const btn = document.createElement('button'); btn.className = 'action-btn'; btn.type = 'button'; btn.textContent = label;
    if (title) btn.title = title; if (onClick) btn.addEventListener('click', onClick); return btn;
  }
  function attachAssistantActions(entry) {
    if (!entry || !entry.row) return;
    const actions = document.createElement('div'); actions.className = 'message-actions';
    actions.appendChild(createActionButton('重试', '重试上一条回答', () => retryLast()));
    actions.appendChild(createActionButton('复制', '复制回答内容', () => copyToClipboard(entry.raw||'')));
    actions.appendChild(createActionButton('反馈', '反馈到 Grok2API', () => window.open(feedbackUrl, '_blank', 'noopener')));
    entry.row.appendChild(actions);
  }

  async function copyToClipboard(text) {
    if (!text) { toast('暂无内容可复制', 'error'); return; }
    try {
      if (navigator.clipboard && navigator.clipboard.writeText) await navigator.clipboard.writeText(text);
      else { const t = document.createElement('textarea'); t.value = text; t.style.position = 'fixed'; t.style.opacity = '0'; document.body.appendChild(t); t.select(); document.execCommand('copy'); document.body.removeChild(t); }
      toast('已复制', 'success');
    } catch (e) { toast('复制失败', 'error'); }
  }

  async function getAuthHeaders() {
    let headers = { 'Content-Type': 'application/json' };
    try {
      const authKey = await ensurePublicKey();
      if (authKey === null) { window.location.href = '/login'; return null; }
      headers = { ...headers, ...buildPublicAuthHeaders(authKey) };
    } catch (e) { /* ignore */ }
    return headers;
  }

  async function retryLast() {
    if (isSending || !messageHistory.length) return;
    let lastUserIndex = -1;
    for (let i = messageHistory.length - 1; i >= 0; i--) { if (messageHistory[i].role === 'user') { lastUserIndex = i; break; } }
    if (lastUserIndex === -1) { toast('没有可重试的对话', 'error'); return; }
    const headers = await getAuthHeaders(); if (!headers) return;
    const assistantEntry = createMessage('assistant', '');
    setSendingState(true); setStatus('connecting', '发送中');
    abortController = new AbortController();
    try {
      const res = await fetch('/v1/chat/completions', { method: 'POST', headers, body: JSON.stringify(buildPayloadFrom(messageHistory.slice(0, lastUserIndex + 1))), signal: abortController.signal });
      if (!res.ok) throw new Error(`请求失败: ${res.status}`);
      await handleStream(res, assistantEntry);
      setStatus('connected', '完成');
    } catch (e) {
      updateMessage(assistantEntry, `请求失败: ${e.message||e}`, true);
      setStatus('error', '失败'); toast('请求失败，请检查服务状态', 'error');
    } finally { setSendingState(false); abortController = null; scrollToBottom(); }
  }

  async function sendMessage() {
    if (isSending) return;
    const prompt = promptInput ? promptInput.value.trim() : '';
    if (!prompt && !attachment) { toast('请输入内容', 'error'); return; }
    let displayText = prompt || '';
    if (attachment) { const label = `[文件] ${attachment.name}`; displayText = displayText ? `${displayText}\n${label}` : label; }
    createMessage('user', displayText);
    let content = prompt;
    if (attachment) {
      const blocks = [];
      if (prompt) blocks.push({ type: 'text', text: prompt });
      blocks.push({ type: 'file', file: { file_data: attachment.data } });
      content = blocks;
    }
    messageHistory.push({ role: 'user', content });
    if (promptInput) promptInput.value = '';
    clearAttachment();
    const headers = await getAuthHeaders(); if (!headers) return;
    const assistantEntry = createMessage('assistant', '');
    setSendingState(true); setStatus('connecting', '发送中');
    abortController = new AbortController();
    try {
      const res = await fetch('/v1/chat/completions', { method: 'POST', headers, body: JSON.stringify(buildPayload()), signal: abortController.signal });
      if (!res.ok) throw new Error(`请求失败: ${res.status}`);
      await handleStream(res, assistantEntry);
      setStatus('connected', '完成');
    } catch (e) {
      if (e && e.name === 'AbortError') {
        updateMessage(assistantEntry, assistantEntry.raw || '已停止', true);
        if (assistantEntry.hasThink) updateThinkSummary(assistantEntry, assistantEntry.thinkElapsed || Math.max(1, Math.round((Date.now()-assistantEntry.startedAt)/1000)));
        setStatus('error', '已停止');
        if (!assistantEntry.committed) { messageHistory.push({ role: 'assistant', content: assistantEntry.raw||'' }); assistantEntry.committed = true; }
      } else {
        updateMessage(assistantEntry, `请求失败: ${e.message||e}`, true);
        setStatus('error', '失败'); toast('请求失败，请检查服务状态', 'error');
      }
    } finally { setSendingState(false); abortController = null; scrollToBottom(); }
  }

  async function handleStream(res, entry) {
    const reader = res.body.getReader(), decoder = new TextDecoder('utf-8');
    let buffer = '', text = '';
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      const parts = buffer.split('\n\n'); buffer = parts.pop() || '';
      for (const part of parts) {
        for (const line of part.split('\n')) {
          const trimmed = line.trim();
          if (!trimmed.startsWith('data:')) continue;
          const payload = trimmed.slice(5).trim();
          if (!payload) continue;
          if (payload === '[DONE]') {
            updateMessage(entry, text, true);
            if (entry.hasThink) updateThinkSummary(entry, entry.thinkElapsed || Math.max(1, Math.round((Date.now()-entry.startedAt)/1000)));
            messageHistory.push({ role: 'assistant', content: text }); entry.committed = true; return;
          }
          try {
            const json = JSON.parse(payload);
            const delta = json&&json.choices&&json.choices[0]&&json.choices[0].delta ? json.choices[0].delta.content : '';
            if (delta) {
              text += delta;
              if (!entry.firstTokenAt) entry.firstTokenAt = Date.now();
              if (!entry.hasThink && text.includes('<think>')) { entry.hasThink = true; entry.thinkElapsed = null; updateThinkSummary(entry, null); }
              updateMessage(entry, text, false);
            }
          } catch (e) { /* ignore */ }
        }
      }
    }
    updateMessage(entry, text, true);
    if (entry.hasThink) updateThinkSummary(entry, entry.thinkElapsed || Math.max(1, Math.round((Date.now()-entry.startedAt)/1000)));
    messageHistory.push({ role: 'assistant', content: text }); entry.committed = true;
  }

  function toggleSettings(show) {
    if (!settingsPanel) return;
    if (typeof show === 'boolean') settingsPanel.classList.toggle('hidden', !show);
    else settingsPanel.classList.toggle('hidden');
  }

  function bindEvents() {
    if (tempRange) tempRange.addEventListener('input', updateRangeValues);
    if (topPRange) topPRange.addEventListener('input', updateRangeValues);
    if (sendBtn) sendBtn.addEventListener('click', sendMessage);
    if (settingsToggle) settingsToggle.addEventListener('click', e => { e.stopPropagation(); toggleSettings(); });
    document.addEventListener('click', e => {
      if (!settingsPanel || settingsPanel.classList.contains('hidden')) return;
      if (settingsPanel.contains(e.target) || (settingsToggle && settingsToggle.contains(e.target))) return;
      toggleSettings(false);
    });
    if (promptInput) {
      let composing = false;
      promptInput.addEventListener('compositionstart', () => { composing = true; });
      promptInput.addEventListener('compositionend', () => { composing = false; });
      promptInput.addEventListener('keydown', e => { if (e.key === 'Enter' && !e.shiftKey && !composing && !e.isComposing) { e.preventDefault(); sendMessage(); } });
    }
    if (attachBtn && fileInput) {
      attachBtn.addEventListener('click', () => fileInput.click());
      fileInput.addEventListener('change', () => { if (fileInput.files && fileInput.files[0]) handleFileSelect(fileInput.files[0]); });
    }
    if (fileRemoveBtn) fileRemoveBtn.addEventListener('click', clearAttachment);
  }

  updateRangeValues();
  loadModels();
  bindEvents();
})();
