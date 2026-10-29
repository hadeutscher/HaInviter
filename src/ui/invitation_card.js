// config is supplied by the Rust component. Prepare before the share click so
// file sharing can start while the browser still has transient user activation.
window.hainviterCards ??= new Map();
const cards = window.hainviterCards;
const previous = cards.get(config.key);
if (previous?.url) URL.revokeObjectURL(previous.url);
const entry = {};
cards.set(config.key, entry);
try {
    const canvas = document.createElement('canvas');
    const ctx = canvas.getContext('2d');
    if (!ctx) throw new Error('Canvas unavailable');
    if (config.image) {
        const response = await fetch(config.image, {signal: AbortSignal.timeout(15000)});
        if (!response.ok) throw new Error('Image unavailable');
        const source = await response.blob();
        if (!source.type.startsWith('image/') || source.size > 10 * 1024 * 1024) {
            throw new Error('Invalid image');
        }
        const bitmap = await createImageBitmap(source);
        try {
            const scale = Math.min(1, 2160 / Math.max(bitmap.width, bitmap.height));
            canvas.width = Math.max(1, Math.round(bitmap.width * scale));
            canvas.height = Math.max(1, Math.round(bitmap.height * scale));
            ctx.fillStyle = '#fff';
            ctx.fillRect(0, 0, canvas.width, canvas.height);
            ctx.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
        } finally {
            bitmap.close();
        }
    } else {
        canvas.width = 1080;
        canvas.height = 1440;
        ctx.fillStyle = '#faf6ee';
        ctx.fillRect(0, 0, 1080, 1440);
        ctx.strokeStyle = '#ac8a52';
        ctx.lineWidth = 3;
        ctx.strokeRect(48, 48, 984, 1344);
        ctx.strokeRect(62, 62, 956, 1316);
        ctx.textAlign = 'center';
        ctx.direction = config.rtl ? 'rtl' : 'ltr';
        // Fit each block, including long titles and venue names, into its area.
        function block(text, top, height, initialSize, color) {
            if (!text.trim()) return;
            let lines, size = initialSize;
            do {
                ctx.font = `${size}px Georgia, 'Times New Roman', serif`;
                lines = [];
                for (const paragraph of text.split('\n')) {
                    let line = '';
                    for (const word of paragraph.split(/\s+/)) {
                        const next = line ? `${line} ${word}` : word;
                        if (ctx.measureText(next).width > 820 && line) {
                            lines.push(line);
                            line = word;
                        } else line = next;
                    }
                    lines.push(line);
                }
                if (lines.length * size * 1.4 <= height) break;
                size -= 2;
            } while (size > 12);
            ctx.fillStyle = color;
            const lineHeight = size * 1.4;
            const y = top + (height - lines.length * lineHeight) / 2 + size;
            lines.forEach((line, i) => ctx.fillText(line, 540, y + i * lineHeight, 820));
        }
        block(config.hosts, 160, 180, 42, '#84683e');
        block(config.title, 380, 400, 78, '#293c35');
        ctx.fillStyle = '#ac8a52';
        ctx.fillRect(420, 835, 240, 3);
        block(config.date, 895, 150, 42, '#293c35');
        block(config.location, 1090, 200, 44, '#293c35');
    }
    const blob = await new Promise(resolve => canvas.toBlob(resolve, 'image/png'));
    if (!blob) throw new Error('Image encoding failed');
    // A dismissed dialog or a retry invalidates an in-flight preparation.
    if (cards.get(config.key) !== entry) return '';
    entry.file = new File([blob], 'invitation.png', {type: 'image/png'});
    entry.url = URL.createObjectURL(blob);
    return entry.url;
} catch (_) {
    if (cards.get(config.key) === entry) cards.delete(config.key);
    return '';
}
