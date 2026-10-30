import { useEffect, useRef, useState } from "react";

export default function CopyButton({ text }: { text: string }) {
  const [label, setLabel] = useState("Copy");
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const active = useRef(false);
  useEffect(() => {
    active.current = true;
    return () => { active.current = false; clearTimeout(timer.current); };
  }, []);
  async function copy() {
    try {
      if (!navigator.clipboard?.writeText) throw new Error("Copy failed");
      await navigator.clipboard.writeText(text);
      if (!active.current) return;
      setLabel("Copied");
      clearTimeout(timer.current);
      timer.current = setTimeout(() => setLabel("Copy"), 2000);
    } catch { if (active.current) setLabel("Copy failed"); }
  }
  return <button type="button" onClick={() => void copy()} aria-live="polite">{label}</button>;
}
