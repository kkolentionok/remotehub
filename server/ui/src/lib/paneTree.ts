/**
 * Pane layout tree for split workspaces. A tab's content is a binary
 * tree: a `leaf` hosts one session (by key); a `split` divides space
 * between two children, horizontally (`row` → side by side) or
 * vertically (`col` → stacked), at `ratio` (fraction for child `a`).
 *
 * All functions here are pure — they return new trees, never mutate.
 */
export type SplitDir = "row" | "col";

export type PaneNode =
    | { t: "leaf"; key: string }
    | { t: "split"; dir: SplitDir; ratio: number; a: PaneNode; b: PaneNode };

/** All session keys referenced by leaves, left-to-right / top-to-bottom. */
export function leafKeys(node: PaneNode): string[] {
    if (node.t === "leaf") return [node.key];
    return [...leafKeys(node.a), ...leafKeys(node.b)];
}

/** First leaf key in the tree (used to pick a fallback focus). */
export function firstLeafKey(node: PaneNode): string {
    return node.t === "leaf" ? node.key : firstLeafKey(node.a);
}

export function hasLeaf(node: PaneNode, key: string): boolean {
    if (node.t === "leaf") return node.key === key;
    return hasLeaf(node.a, key) || hasLeaf(node.b, key);
}

/**
 * Replace the leaf identified by `targetKey` with a split that keeps the
 * existing session in child `a` and the new one in child `b`.
 */
export function splitLeaf(
    node: PaneNode,
    targetKey: string,
    newKey: string,
    dir: SplitDir,
): PaneNode {
    if (node.t === "leaf") {
        if (node.key !== targetKey) return node;
        return {
            t: "split",
            dir,
            ratio: 0.5,
            a: { t: "leaf", key: targetKey },
            b: { t: "leaf", key: newKey },
        };
    }
    return {
        ...node,
        a: splitLeaf(node.a, targetKey, newKey, dir),
        b: splitLeaf(node.b, targetKey, newKey, dir),
    };
}

/**
 * Remove the leaf with `key`. A split that loses a child collapses to
 * its surviving child. Returns `null` if the whole tree is removed.
 */
export function removeLeaf(node: PaneNode, key: string): PaneNode | null {
    if (node.t === "leaf") return node.key === key ? null : node;
    const a = removeLeaf(node.a, key);
    const b = removeLeaf(node.b, key);
    if (a === null) return b;
    if (b === null) return a;
    return { ...node, a, b };
}

/**
 * Like `splitLeaf`, but controls order: when `newFirst` the incoming pane
 * takes child `a` (left/top), otherwise `b` (right/bottom).
 */
export function splitLeafWith(
    node: PaneNode,
    targetKey: string,
    newKey: string,
    dir: SplitDir,
    newFirst: boolean,
): PaneNode {
    if (node.t === "leaf") {
        if (node.key !== targetKey) return node;
        const existing: PaneNode = { t: "leaf", key: targetKey };
        const incoming: PaneNode = { t: "leaf", key: newKey };
        return {
            t: "split",
            dir,
            ratio: 0.5,
            a: newFirst ? incoming : existing,
            b: newFirst ? existing : incoming,
        };
    }
    return {
        ...node,
        a: splitLeafWith(node.a, targetKey, newKey, dir, newFirst),
        b: splitLeafWith(node.b, targetKey, newKey, dir, newFirst),
    };
}

/**
 * Set the split ratio of the node whose direct children are exactly the
 * two given subtrees. We address splits by an opaque path of "a"/"b"
 * steps from the root, which the renderer knows for each divider.
 */
export function setRatioAtPath(
    node: PaneNode,
    path: ("a" | "b")[],
    ratio: number,
): PaneNode {
    if (path.length === 0) {
        if (node.t !== "split") return node;
        return { ...node, ratio: Math.min(0.85, Math.max(0.15, ratio)) };
    }
    if (node.t !== "split") return node;
    const [head, ...rest] = path;
    if (head === "a") return { ...node, a: setRatioAtPath(node.a, rest, ratio) };
    return { ...node, b: setRatioAtPath(node.b, rest, ratio) };
}
