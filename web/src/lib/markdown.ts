// Markdown, as far as agents use it: headings, lists, quotes, code, links,
// and [[links to other notes]]. Everything is escaped first, so what comes
// out is safe to put in the page.

const escape = (text: string) =>
  text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

/** Code, bold, italics and links inside a line. `text` is already escaped. */
function inline(text: string): string {
  const codes: string[] = [];
  return text
    .replace(/`([^`\n]+)`/g, (_, code) => `\u0000${codes.push(code) - 1}\u0000`)
    .replace(/\[\[([^\]\n|]+)(?:\|([^\]\n]+))?\]\]/g, (_, to, shown) => `<a href="#" data-note="${to.trim()}">${shown ?? to}</a>`)
    .replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, (_, shown, to) =>
      /^https?:\/\//.test(to)
        ? `<a href="${to}" target="_blank" rel="noreferrer">${shown}</a>`
        : `<a href="#" data-path="${to}">${shown}</a>`,
    )
    .replace(/(^|[\s(])(https?:\/\/[^\s<)]+[^\s<).,;:!?])/g, '$1<a href="$2" target="_blank" rel="noreferrer">$2</a>')
    .replace(/\*\*([^*\n]+)\*\*/g, '<strong>$1</strong>')
    .replace(/(^|[\s(])\*([^*\s][^*\n]*)\*(?=[\s).,;:!?]|$)/g, '$1<em>$2</em>')
    .replace(/\u0000(\d+)\u0000/g, (_, i) => `<code>${codes[Number(i)]}</code>`);
}

/** What agents write while working: code blocks and inline marks, line breaks kept. */
export function light(text: string): string {
  return escape(text)
    .split(/```[^\n]*\n?([\s\S]*?)(?:```|$)/g)
    .map((part, i) => (i % 2 ? `<pre>${part.replace(/\n$/, '')}</pre>` : inline(part)))
    .join('');
}

/** A whole document. */
export function document(text: string): string {
  const lines = escape(text.replace(/\r\n/g, '\n')).split('\n');
  const out: string[] = [];
  let paragraph: string[] = [];
  let list: { tag: 'ul' | 'ol'; items: string[] } | null = null;
  const flush = () => {
    if (paragraph.length) out.push(`<p>${inline(paragraph.join(' '))}</p>`);
    if (list) out.push(`<${list.tag}>${list.items.map((item) => `<li>${inline(item)}</li>`).join('')}</${list.tag}>`);
    paragraph = [];
    list = null;
  };
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (/^\s*```/.test(line)) {
      flush();
      const code: string[] = [];
      for (i++; i < lines.length && !/^\s*```/.test(lines[i]); i++) code.push(lines[i]);
      out.push(`<pre>${code.join('\n')}</pre>`);
      continue;
    }
    const heading = /^(#{1,6})\s+(.*)$/.exec(line);
    const item = /^\s*(?:([-*+])|(\d+)[.)])\s+(.*)$/.exec(line);
    if (heading) {
      flush();
      const level = heading[1].length;
      out.push(`<h${level}>${inline(heading[2])}</h${level}>`);
    } else if (/^\s*(?:---+|\*\*\*+)\s*$/.test(line)) {
      flush();
      out.push('<hr>');
    } else if (item) {
      const tag = item[1] ? 'ul' : 'ol';
      if (paragraph.length || (list && list.tag !== tag)) flush();
      list ??= { tag, items: [] };
      list.items.push(item[3]);
    } else if (/^&gt;\s?/.test(line)) {
      flush();
      const quote: string[] = [];
      for (; i < lines.length && /^&gt;\s?/.test(lines[i]); i++) quote.push(lines[i].replace(/^&gt;\s?/, ''));
      i--;
      out.push(`<blockquote>${inline(quote.join(' '))}</blockquote>`);
    } else if (!line.trim()) {
      flush();
    } else if (list && /^\s{2,}\S/.test(line)) {
      // A list item that goes on over the next line.
      list.items[list.items.length - 1] += ' ' + line.trim();
    } else {
      if (list) flush();
      paragraph.push(line.trim());
    }
  }
  flush();
  return out.join('\n');
}
