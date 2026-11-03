// Small dialogs (names, confirmations) as promises; rendered by PromptDialog.svelte.
export interface Field {
  name: string;
  label: string;
  value?: string;
  type?: "text" | "password" | "select";
  options?: { value: string; label: string }[];
  required?: boolean;
  pattern?: string;
  placeholder?: string;
  hint?: string;
}

export interface Ask {
  title: string;
  message?: string;
  fields: Field[];
  submit: string;
  danger?: boolean;
}

interface Pending extends Ask {
  resolve: (values: Record<string, string> | null) => void;
}

class PromptState {
  current = $state<Pending | null>(null);

  /** Resolves to the field values, or null when cancelled. */
  ask(a: Ask): Promise<Record<string, string> | null> {
    this.current?.resolve(null);
    return new Promise((resolve) => (this.current = { ...a, resolve }));
  }

  async confirm(title: string, message: string, submit = "Delete"): Promise<boolean> {
    return (await this.ask({ title, message, fields: [], submit, danger: true })) !== null;
  }

  finish(values: Record<string, string> | null) {
    const p = this.current;
    this.current = null;
    p?.resolve(values);
  }
}

export const prompt = new PromptState();

/** A file or folder name in a collection (`valid_segment` on the server). */
export const NAME_PATTERN = "[A-Za-z0-9_\\-][A-Za-z0-9_.\\-]*";
