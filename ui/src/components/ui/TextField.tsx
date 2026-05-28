import type { InputHTMLAttributes, TextareaHTMLAttributes } from "react";
import styles from "./TextField.module.css";

interface TextFieldProps {
    label: string;
    hint?: string;
    error?: string;
    children: React.ReactNode;
}

/**
 * Form field shell — label + hint + error slot.
 * Wrap an <input> or <textarea> inside.
 */
export function TextField({ label, hint, error, children }: TextFieldProps) {
    return (
        <label className={styles.field}>
            <span className={styles.label}>{label}</span>
            {children}
            {error ? (
                <span className={styles.error}>{error}</span>
            ) : hint ? (
                <span className={styles.hint}>{hint}</span>
            ) : null}
        </label>
    );
}

export function Input(props: InputHTMLAttributes<HTMLInputElement>) {
    const { className, ...rest } = props;
    return <input className={`${styles.input} ${className ?? ""}`} {...rest} />;
}

export function Textarea(props: TextareaHTMLAttributes<HTMLTextAreaElement>) {
    const { className, ...rest } = props;
    return <textarea className={`${styles.textarea} ${className ?? ""}`} {...rest} />;
}

export function Select(props: React.SelectHTMLAttributes<HTMLSelectElement>) {
    const { className, children, ...rest } = props;
    return (
        <select className={`${styles.select} ${className ?? ""}`} {...rest}>
            {children}
        </select>
    );
}
