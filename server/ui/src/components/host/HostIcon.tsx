import type { ComponentType } from "react";
import { Monitor, Server } from "lucide-react";
import {
    SiAlmalinux,
    SiAlpinelinux,
    SiApple,
    SiArchlinux,
    SiCentos,
    SiDebian,
    SiFedora,
    SiFreebsd,
    SiGentoo,
    SiKalilinux,
    SiLinux,
    SiLinuxmint,
    SiManjaro,
    SiOpenbsd,
    SiOpensuse,
    SiPopos,
    SiRaspberrypi,
    SiRedhat,
    SiRockylinux,
    SiUbuntu,
    SiVoidlinux,
} from "react-icons/si";

import styles from "../sidebar/Sidebar.module.css";

type IconCmp = ComponentType<{ size?: string | number }>;

/**
 * Map a detected-OS slug (from `/etc/os-release` ID, or our `uname`
 * heuristic) to a Simple Icons brand glyph. Returns `null` for things
 * with no good logo so the caller can fall back to a generic icon.
 *
 * Rendered MONOCHROME (currentColor) — per the design language we don't
 * use saturated brand colors anywhere but the accent. The glyph just
 * carries shape recognition, not brand color.
 */
export function osIcon(slug: string): IconCmp | null {
    const s = slug.toLowerCase();
    // Distros whose os-release ID matches a known logo.
    const exact: Record<string, IconCmp> = {
        ubuntu: SiUbuntu,
        debian: SiDebian,
        raspbian: SiRaspberrypi,
        fedora: SiFedora,
        arch: SiArchlinux,
        archlinux: SiArchlinux,
        manjaro: SiManjaro,
        alpine: SiAlpinelinux,
        centos: SiCentos,
        rhel: SiRedhat,
        redhat: SiRedhat,
        rocky: SiRockylinux,
        almalinux: SiAlmalinux,
        kali: SiKalilinux,
        gentoo: SiGentoo,
        void: SiVoidlinux,
        pop: SiPopos,
        linuxmint: SiLinuxmint,
        mint: SiLinuxmint,
        freebsd: SiFreebsd,
        openbsd: SiOpenbsd,
        macos: SiApple,
        darwin: SiApple,
    };
    if (exact[s]) return exact[s];
    if (s.startsWith("opensuse") || s === "sles" || s === "suse") return SiOpensuse;
    if (s === "windows") return Monitor; // Simple Icons has no Windows mark
    // Anything else that came from os-release is still Linux → Tux.
    if (s.length > 0 && s !== "unknown") return SiLinux;
    return null;
}

/**
 * Host icon slot in the sidebar (and elsewhere). Shows an OS-specific
 * Simple Icons glyph once `detectedOs` is populated (after the first
 * successful SSH connect), otherwise a generic server.
 */
export function HostIcon({ detectedOs }: { detectedOs?: string | null }) {
    const Icon: IconCmp = (detectedOs && osIcon(detectedOs)) || Server;
    return (
        <span className={styles.hostIcon} title={detectedOs ?? undefined}>
            <Icon size={14} />
        </span>
    );
}
