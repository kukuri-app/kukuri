/* eslint-disable react-refresh/only-export-components */
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from 'react';
import { createPortal } from 'react-dom';

import { cn } from '@/lib/utils';

export type ContextActionMenuItem = {
  id: string;
  label: string;
  onSelect: () => void | Promise<void>;
  tone?: 'default' | 'danger';
  disabled?: boolean;
};

export type ContextActionMenuPosition = {
  x: number;
  y: number;
  returnFocusTo?: HTMLElement | null;
};

const INTERACTIVE_DESCENDANT_SELECTOR =
  'button, a, input, select, textarea, [role="button"], [role="link"], [role="menuitem"]';

function isFromInteractiveDescendant(
  target: EventTarget | null,
  currentTarget: HTMLElement
): boolean {
  if (!(target instanceof Element) || target === currentTarget) {
    return false;
  }
  const closestInteractiveElement = target.closest(INTERACTIVE_DESCENDANT_SELECTOR);
  return closestInteractiveElement !== null && closestInteractiveElement !== currentTarget;
}

export function contextActionMenuPositionFromPointer(
  event: ReactMouseEvent<HTMLElement>
): ContextActionMenuPosition | null {
  if (isFromInteractiveDescendant(event.target, event.currentTarget)) {
    return null;
  }
  event.preventDefault();
  event.stopPropagation();
  return {
    x: event.clientX,
    y: event.clientY,
    returnFocusTo:
      event.target instanceof HTMLElement && event.target.tabIndex >= 0
        ? event.target
        : event.currentTarget,
  };
}

export function contextActionMenuPositionFromKeyboard(
  event: ReactKeyboardEvent<HTMLElement>
): ContextActionMenuPosition | null {
  if (event.key !== 'ContextMenu' && !(event.key === 'F10' && event.shiftKey)) {
    return null;
  }
  if (isFromInteractiveDescendant(event.target, event.currentTarget)) {
    return null;
  }
  event.preventDefault();
  event.stopPropagation();
  const rect = event.currentTarget.getBoundingClientRect();
  return {
    x: rect.left + Math.min(24, Math.max(rect.width / 2, 0)),
    y: rect.top + Math.min(24, Math.max(rect.height, 0)),
    returnFocusTo:
      event.target instanceof HTMLElement && event.target.tabIndex >= 0
        ? event.target
        : event.currentTarget,
  };
}

type ContextActionMenuProps = {
  open: boolean;
  position: ContextActionMenuPosition | null;
  items: ContextActionMenuItem[];
  onClose: () => void;
};

const VIEWPORT_GUTTER_PX = 8;
const FALLBACK_MENU_WIDTH_PX = 180;
const FALLBACK_MENU_HEIGHT_PX = 120;

export function ContextActionMenu({
  open,
  position,
  items,
  onClose,
}: ContextActionMenuProps) {
  const menuRef = useRef<HTMLDivElement | null>(null);
  const itemRefs = useRef<Array<HTMLButtonElement | null>>([]);
  const [menuSize, setMenuSize] = useState({
    width: FALLBACK_MENU_WIDTH_PX,
    height: FALLBACK_MENU_HEIGHT_PX,
  });

  const closeMenu = useCallback(
    (restoreFocus: boolean) => {
      const returnFocusTo = position?.returnFocusTo;
      if (restoreFocus && returnFocusTo?.isConnected) {
        returnFocusTo.focus();
      }
      onClose();
    },
    [onClose, position?.returnFocusTo]
  );

  useEffect(() => {
    if (!open) {
      return undefined;
    }

    const handlePointerDown = (event: PointerEvent) => {
      if (menuRef.current?.contains(event.target as Node)) {
        return;
      }
      closeMenu(false);
    };

    const handleResize = () => closeMenu(true);

    window.addEventListener('pointerdown', handlePointerDown);
    window.addEventListener('resize', handleResize);

    return () => {
      window.removeEventListener('pointerdown', handlePointerDown);
      window.removeEventListener('resize', handleResize);
    };
  }, [closeMenu, open]);

  useEffect(() => {
    if (!open || !menuRef.current) {
      return;
    }
    const rect = menuRef.current.getBoundingClientRect();
    setMenuSize({
      width: rect.width || FALLBACK_MENU_WIDTH_PX,
      height: rect.height || FALLBACK_MENU_HEIGHT_PX,
    });
    // 最初の focus は開いた時と位置が変わった時だけ移す。持ち主の再描画で items が作り直されても、
    // keyboard で移した focus を先頭へ戻さない。
    (itemRefs.current.find((item) => item && !item.disabled) ?? menuRef.current).focus();
  }, [open, position?.x, position?.y]);

  const menuStyle = useMemo(() => {
    if (!position || typeof window === 'undefined') {
      return undefined;
    }
    const left = Math.max(
      VIEWPORT_GUTTER_PX,
      Math.min(position.x, window.innerWidth - menuSize.width - VIEWPORT_GUTTER_PX)
    );
    const top = Math.max(
      VIEWPORT_GUTTER_PX,
      Math.min(position.y, window.innerHeight - menuSize.height - VIEWPORT_GUTTER_PX)
    );
    return {
      left,
      top,
    };
  }, [menuSize.height, menuSize.width, position]);

  if (!open || !position || typeof document === 'undefined') {
    return null;
  }

  return createPortal(
    <div
      ref={menuRef}
      role='menu'
      tabIndex={-1}
      className='context-action-menu panel'
      style={menuStyle}
      onContextMenu={(event) => event.preventDefault()}
      onKeyDown={(event) => {
        if (event.key === 'Escape' || event.key === 'Tab') {
          event.preventDefault();
          closeMenu(true);
          return;
        }
        const enabledIndexes = items.flatMap((item, index) => (item.disabled ? [] : [index]));
        if (enabledIndexes.length === 0) return;
        const currentIndex = itemRefs.current.findIndex((item) => item === document.activeElement);
        const currentEnabledPosition = enabledIndexes.indexOf(currentIndex);
        let nextIndex: number | null = null;
        if (event.key === 'ArrowDown') {
          nextIndex = enabledIndexes[(currentEnabledPosition + 1) % enabledIndexes.length];
        } else if (event.key === 'ArrowUp') {
          nextIndex = enabledIndexes[
            (currentEnabledPosition - 1 + enabledIndexes.length) % enabledIndexes.length
          ];
        } else if (event.key === 'Home') {
          nextIndex = enabledIndexes[0];
        } else if (event.key === 'End') {
          nextIndex = enabledIndexes.at(-1) ?? null;
        }
        if (nextIndex !== null) {
          event.preventDefault();
          itemRefs.current[nextIndex]?.focus();
        }
      }}
    >
      {items.map((item, index) => (
        <button
          key={item.id}
          ref={(element) => {
            itemRefs.current[index] = element;
          }}
          type='button'
          role='menuitem'
          disabled={item.disabled}
          className={cn(
            'context-action-menu-item',
            item.tone === 'danger' && 'context-action-menu-item-danger'
          )}
          onClick={async () => {
            try {
              await item.onSelect();
            } finally {
              closeMenu(true);
            }
          }}
        >
          {item.label}
        </button>
      ))}
    </div>,
    document.body
  );
}
