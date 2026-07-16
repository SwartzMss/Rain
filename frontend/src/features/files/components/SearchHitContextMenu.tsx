import { useEffect, useRef, useState } from 'react';
import { placeContextMenu } from '../searchHitSource';

type SearchHitContextMenuProps = {
  x: number;
  y: number;
  onOpen: () => void;
  onClose: () => void;
};

export function SearchHitContextMenu({
  x,
  y,
  onOpen,
  onClose
}: SearchHitContextMenuProps) {
  const menuRef = useRef<HTMLDivElement | null>(null);
  const [position, setPosition] = useState(() => ({ left: x, top: y }));

  useEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    setPosition(placeContextMenu(
      { x, y },
      { width: menu.offsetWidth, height: menu.offsetHeight },
      { width: window.innerWidth, height: window.innerHeight }
    ));
    const firstEnabled = menu.querySelector<HTMLButtonElement>('button:not(:disabled)');
    firstEnabled?.focus();
  }, [x, y]);

  useEffect(() => {
    const closeFromOutside = (event: PointerEvent) => {
      if (!menuRef.current?.contains(event.target as Node)) onClose();
    };
    const closeFromScroll = () => onClose();
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onClose();
        return;
      }
      if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
      const buttons = Array.from(
        menuRef.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)') ?? []
      );
      if (buttons.length === 0) return;
      event.preventDefault();
      const current = buttons.indexOf(document.activeElement as HTMLButtonElement);
      const delta = event.key === 'ArrowDown' ? 1 : -1;
      buttons[(current + delta + buttons.length) % buttons.length]?.focus();
    };
    window.addEventListener('pointerdown', closeFromOutside);
    window.addEventListener('scroll', closeFromScroll, true);
    window.addEventListener('keydown', handleKeyDown);
    return () => {
      window.removeEventListener('pointerdown', closeFromOutside);
      window.removeEventListener('scroll', closeFromScroll, true);
      window.removeEventListener('keydown', handleKeyDown);
    };
  }, [onClose]);

  const run = (action: () => void) => {
    action();
    onClose();
  };

  return (
    <div
      ref={menuRef}
      role="menu"
      aria-label="搜索结果操作"
      className="fixed z-50 min-w-48 overflow-hidden rounded-lg border border-slate-200 bg-white p-1 text-sm shadow-xl"
      style={position}
    >
      <button
        type="button"
        role="menuitem"
        className="block w-full rounded px-3 py-2 text-left text-slate-700 hover:bg-sky-50 hover:text-sky-800 focus:bg-sky-50 focus:outline-none"
        onClick={() => run(onOpen)}
      >
        在原文件中打开
      </button>
    </div>
  );
}
