// Render demo lines (src/data/demo.mjs) to HTML for the terminal and the How it works stages.
type Span = { text: string; tone?: string; bold?: boolean };
export const esc = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
// Geist Mono has no glyph for these, so each sits in a 1ch box and the columns stay true.
export const glyphs = (text: string) => esc(text).replace(/[^\x00-\x7E·…—]/g, c => `<i class="g">${c}</i>`);
export const spans = (line: Span[]) => line.map(span => {
 const cls = [span.tone && `c-${span.tone}`, span.bold && 'b'].filter(Boolean).join(' ');
 return cls ? `<span class="${cls}">${glyphs(span.text)}</span>` : glyphs(span.text);
}).join('');
