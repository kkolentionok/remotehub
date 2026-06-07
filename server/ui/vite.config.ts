import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// https://vitejs.dev/config/
export default defineConfig({
    plugins: [react()],

    // Tauri expects a fixed dev port and prevents Vite from clearing the
    // screen so we can see tauri output.
    clearScreen: false,

    server: {
        port: 1420,
        strictPort: true,
        host: false,
    },

    envPrefix: ["VITE_", "TAURI_"],

    build: {
        // Tauri uses Chromium on Windows and WebKit on macOS/Linux.
        target: ["es2021", "chrome105", "safari14"],
        minify: !process.env.TAURI_DEBUG ? "esbuild" : false,
        sourcemap: !!process.env.TAURI_DEBUG,
    },
});
