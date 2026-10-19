import { createElement, Fragment } from 'react';
import { MemoryRouter } from 'react-router-dom';

import type { Preview } from '@storybook/react-vite';
import { sb } from 'storybook/test';

import i18n, { type SupportedLocale } from '@/i18n';
import { installWindowDesktopMock } from '@/mocks/installWindowDesktopMock';

import '@/styles/index.css';

sb.mock('../src/lib/api/osNotificationPermission.ts', { spy: true });
sb.mock('../src/lib/api/invoke/desktop.ts', { spy: true });
sb.mock('../src/lib/releaseReadiness.ts', { spy: true });

installWindowDesktopMock();

const preview: Preview = {
  globalTypes: {
    theme: {
      name: 'Theme',
      description: 'Desktop shell theme',
      defaultValue: 'dark',
      toolbar: {
        icon: 'mirror',
        items: [
          { value: 'dark', title: 'Dark' },
          { value: 'light', title: 'Light' },
        ],
      },
    },
    shellWidth: {
      name: 'Shell Width',
      description: 'Desktop shell review width',
      defaultValue: 'desktop',
      toolbar: {
        icon: 'browser',
        items: [
          { value: 'narrow', title: 'Narrow desktop' },
          { value: 'desktop', title: 'Desktop' },
          { value: 'mobile375', title: 'Mobile 375' },
          { value: 'mobile390', title: 'Mobile 390' },
          { value: 'mobile430', title: 'Mobile 430' },
          { value: 'compact760', title: 'Desktop 760' },
          { value: 'standard1024', title: 'Desktop 1024' },
          { value: 'wide1280', title: 'Desktop 1280' },
          { value: 'review1440', title: 'Desktop 1440' },
          { value: 'ultrawide1920', title: 'Ultrawide 1920' },
        ],
      },
    },
    motion: {
      name: 'Motion',
      description: 'Motion preference used by review surfaces',
      defaultValue: 'full',
      toolbar: {
        icon: 'lightning',
        items: [
          { value: 'full', title: 'Full motion' },
          { value: 'reduce', title: 'Reduced motion' },
        ],
      },
    },
    locale: {
      name: 'Locale',
      description: 'Desktop shell locale',
      defaultValue: 'en',
      toolbar: {
        icon: 'globe',
        items: [
          { value: 'en', title: 'English' },
          { value: 'ja', title: '日本語' },
          { value: 'zh-CN', title: '简体中文' }
        ]
      }
    }
  },
  parameters: {
    layout: 'fullscreen',
    controls: {
      expanded: true,
    },
  },
  decorators: [
    (Story, context) => {
      const theme = context.globals.theme === 'light' ? 'light' : 'dark';
      const shellWidths: Record<string, number> = {
        narrow: 420,
        desktop: 960,
        mobile375: 375,
        mobile390: 390,
        mobile430: 430,
        compact760: 760,
        standard1024: 1024,
        wide1280: 1280,
        review1440: 1440,
        ultrawide1920: 1920,
      };
      const shellWidth = shellWidths[String(context.globals.shellWidth)] ?? 960;
      const reviewCanvas = context.parameters.reviewCanvas === true;
      const motion = context.globals.motion === 'reduce' ? 'reduce' : 'full';
      const locale = (context.globals.locale ?? 'en') as SupportedLocale;
      document.documentElement.dataset.theme = theme;
      document.documentElement.dataset.reducedMotion = motion;
      document.documentElement.lang = locale;
      void i18n.changeLanguage(locale);

      return createElement(
        'div',
        {
          style: {
            minHeight: '100vh',
            padding: reviewCanvas ? 0 : '24px',
            background: 'var(--shell-background)',
          },
        },
        createElement(
          context.parameters.ownRouter ? Fragment : MemoryRouter,
          null,
          createElement(
            'div',
            {
              className: 'shell-phase1',
              style: {
                width: '100%',
                maxWidth: `${shellWidth}px`,
                margin: '0 auto',
                padding: 0,
                gap: 0,
              },
            },
            createElement(Story),
          ),
        ),
      );
    },
  ],
};

export default preview;
