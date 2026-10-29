const entry = window.hainviterCards?.get(key);
if (!entry?.file) return 'failed';
try {
    if (!navigator.share || !navigator.canShare?.({files: [entry.file]})) return 'unsupported';
    await navigator.share({files: [entry.file], text});
    return 'shared';
} catch (error) {
    return error?.name === 'AbortError' ? 'cancelled' : 'unsupported';
}
