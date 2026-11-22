/**
 * @fileoverview Formatting utilities for discord-context-mcp.
 * Generates LLM-friendly Markdown strings and MCP-compliant content structures.
 */

/**
 * Formats an array of message rows into clean, LLM-friendly Markdown.
 *
 * @param {Array<Object>} messages
 * @param {Object} [options={}]
 * @param {string} [options.title='Messages']
 * @returns {string}
 */
export function formatMessages(messages, { title = 'Messages' } = {}) {
  const list = Array.isArray(messages) ? messages : [];
  const lines = [
    `# ${title} (${list.length})`,
    ''
  ];

  if (list.length === 0) {
    lines.push('_No messages found._');
    return lines.join('\n');
  }

  for (const m of list) {
    const authorName = m.author_global_name || m.author_username || 'Unknown';
    const authorId = m.author_id ? ` (${m.author_id})` : '';

    let timeStr = 'Unknown Time';
    if (m.timestamp) {
      try {
        timeStr = new Date(m.timestamp).toISOString();
      } catch {
        timeStr = String(m.timestamp);
      }
    } else if (m.created_at_ms) {
      timeStr = new Date(m.created_at_ms).toISOString();
    }

    const edited = m.edited_timestamp ? ' (edited)' : '';
    const content = m.content || '';

    lines.push(`[${timeStr}] ${authorName}${authorId}: ${content}${edited}`);

    // Parse attachments if present
    let attachments = [];
    if (m.attachments_json) {
      try {
        attachments = typeof m.attachments_json === 'string'
          ? JSON.parse(m.attachments_json)
          : m.attachments_json;
      } catch {
        // ignore
      }
    }

    if (Array.isArray(attachments) && attachments.length > 0) {
      for (const att of attachments) {
        const url = att.url || att.proxy_url;
        const filename = att.filename || 'attachment';
        if (url) {
          lines.push(`  ↳ [Attachment: ${filename}](${url})`);
        }
      }
    }
  }

  return lines.join('\n');
}

/**
 * Formats an array of plain objects into Markdown list/table.
 *
 * @param {Array<Object>} items
 * @param {Object} [options={}]
 * @param {string} [options.title='Items']
 * @param {Array<string>} [options.fields]
 * @returns {string}
 */
export function formatItems(items, { title = 'Items', fields } = {}) {
  const list = Array.isArray(items) ? items : [];
  const lines = [
    `# ${title} (${list.length})`,
    ''
  ];

  if (list.length === 0) {
    lines.push('_No items found._');
    return lines.join('\n');
  }

  const selectedFields = fields && fields.length > 0
    ? fields
    : Object.keys(list[0] || {});

  for (const item of list) {
    const parts = selectedFields.map((f) => {
      const val = item[f] !== undefined && item[f] !== null ? item[f] : '';
      return `**${f}**: ${val}`;
    });
    lines.push(`- ${parts.join(' | ')}`);
  }

  return lines.join('\n');
}

/**
 * Creates an MCP-shaped text result object.
 *
 * @param {string} text
 * @returns {{ content: Array<{ type: 'text', text: string }>, isError: false }}
 */
export function textResult(text) {
  return {
    content: [
      {
        type: 'text',
        text: String(text ?? '')
      }
    ],
    isError: false
  };
}

/**
 * Creates an MCP-shaped JSON result object.
 *
 * @param {any} obj
 * @returns {{ content: Array<{ type: 'text', text: string }>, isError: false }}
 */
export function jsonResult(obj) {
  return {
    content: [
      {
        type: 'text',
        text: JSON.stringify(obj, null, 2)
      }
    ],
    isError: false
  };
}
