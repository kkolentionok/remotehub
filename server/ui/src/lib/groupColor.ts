/**
 * Group label colors.
 *
 * The data model doesn't store a per-group color yet, so we derive a
 * stable one from the group id: same group → same swatch across the UI,
 * no schema change. When a real `color` column lands, swap `groupColor`
 * to read it and fall back to this.
 *
 * Palette matches the design's group swatches.
 */
export const GROUP_PALETTE = [
    "#4c8eff",
    "#4ade80",
    "#f59e0b",
    "#a78bfa",
    "#f472b6",
    "#22d3ee",
    "#fb7185",
    "#94a3b8",
] as const;

/** Stable swatch for a group id (FNV-1a hash → palette index). */
export function groupColor(id: string | null | undefined): string {
    if (!id) return "#94a3b8";
    let h = 0x811c9dc5;
    for (let i = 0; i < id.length; i++) {
        h ^= id.charCodeAt(i);
        h = Math.imul(h, 0x01000193);
    }
    const idx = (h >>> 0) % GROUP_PALETTE.length;
    return GROUP_PALETTE[idx]!;
}
