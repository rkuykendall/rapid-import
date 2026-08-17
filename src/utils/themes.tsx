export enum Theme {
  Dark = 'dark',
  Light = 'light',
  Grey = 'grey',
  MutedGreen = 'muted-green',
  Blue = 'blue',
  Sepia = 'sepia',
  Snow = 'snow',
  Arctic = 'arctic',
}

export interface ThemeProps {
  cssVariables: Record<string, string>;
  id: Theme;
  name: string;
}

// Dark/Light/Grey values are shared verbatim with RapidRAW's src/utils/themes.ts
// (variable names and format included: flat `rgb()` strings on `--app-*` vars,
// consumed by styles.css's `@theme` block as `--color-x: var(--app-x)`) so the
// two apps look and feel like the same family of tools. The remaining themes
// aren't part of upstream RapidRAW — they're additions kept from this app's
// history — updated to the same var naming/format for consistency.
export const THEMES: Array<ThemeProps> = [
  {
    id: Theme.Dark,
    name: 'Dark',
    cssVariables: {
      '--app-bg-primary': 'rgb(24, 24, 24)',
      '--app-bg-secondary': 'rgb(35, 35, 35)',
      '--app-surface': 'rgb(28, 28, 28)',
      '--app-card-active': 'rgb(43, 43, 43)',
      '--app-button-text': 'rgb(0, 0, 0)',
      '--app-text-primary': 'rgb(232, 234, 237)',
      '--app-text-secondary': 'rgb(158, 158, 158)',
      '--app-accent': 'rgb(255, 255, 255)',
      '--app-border-color': 'rgb(45, 45, 45)',
      '--app-hover-color': 'rgb(255, 255, 255)',
    },
  },
  {
    id: Theme.Light,
    name: 'Light',
    cssVariables: {
      '--app-bg-primary': 'rgb(245, 245, 245)',
      '--app-bg-secondary': 'rgb(255, 255, 255)',
      '--app-surface': 'rgb(241, 241, 241)',
      '--app-card-active': 'rgb(250, 250, 250)',
      '--app-button-text': 'rgb(255, 255, 255)',
      '--app-text-primary': 'rgb(20, 20, 20)',
      '--app-text-secondary': 'rgb(108, 108, 108)',
      '--app-accent': 'rgb(198, 142, 110)',
      '--app-border-color': 'rgb(224, 224, 224)',
      '--app-hover-color': 'rgb(198, 142, 110)',
    },
  },
  {
    id: Theme.Grey,
    name: 'Grey',
    cssVariables: {
      '--app-bg-primary': 'rgb(112, 112, 112)',
      '--app-bg-secondary': 'rgb(118, 118, 118)',
      '--app-surface': 'rgb(108, 108, 108)',
      '--app-card-active': 'rgb(133, 133, 133)',
      '--app-button-text': 'rgb(45, 45, 45)',
      '--app-text-primary': 'rgb(240, 240, 240)',
      '--app-text-secondary': 'rgb(180, 180, 180)',
      '--app-accent': 'rgb(220, 220, 220)',
      '--app-border-color': 'rgb(138, 138, 138)',
      '--app-hover-color': 'rgb(220, 220, 220)',
    },
  },
  {
    id: Theme.MutedGreen,
    name: 'Muted Green',
    cssVariables: {
      '--app-bg-primary': 'rgb(55, 60, 50)',
      '--app-bg-secondary': 'rgb(65, 70, 60)',
      '--app-surface': 'rgb(45, 50, 40)',
      '--app-card-active': 'rgb(75, 80, 70)',
      '--app-button-text': 'rgb(45, 50, 40)',
      '--app-text-primary': 'rgb(227, 225, 220)',
      '--app-text-secondary': 'rgb(155, 160, 150)',
      '--app-accent': 'rgb(219, 212, 173)',
      '--app-border-color': 'rgb(85, 90, 80)',
      '--app-hover-color': 'rgb(219, 212, 173)',
    },
  },
  {
    id: Theme.Blue,
    name: 'Blue',
    cssVariables: {
      '--app-bg-primary': 'rgb(32, 36, 37)',
      '--app-bg-secondary': 'rgb(42, 46, 50)',
      '--app-surface': 'rgb(35, 38, 41)',
      '--app-card-active': 'rgb(52, 57, 62)',
      '--app-button-text': 'rgb(35, 38, 41)',
      '--app-text-primary': 'rgb(220, 225, 230)',
      '--app-text-secondary': 'rgb(145, 155, 165)',
      '--app-accent': 'rgb(152, 187, 199)',
      '--app-border-color': 'rgb(60, 65, 70)',
      '--app-hover-color': 'rgb(152, 187, 199)',
    },
  },
  {
    id: Theme.Sepia,
    name: 'Sepia',
    cssVariables: {
      '--app-bg-primary': 'rgb(48, 43, 38)',
      '--app-bg-secondary': 'rgb(65, 60, 55)',
      '--app-surface': 'rgb(52, 47, 43)',
      '--app-card-active': 'rgb(80, 75, 70)',
      '--app-button-text': 'rgb(50, 45, 40)',
      '--app-text-primary': 'rgb(225, 215, 205)',
      '--app-text-secondary': 'rgb(160, 150, 140)',
      '--app-accent': 'rgb(255, 226, 182)',
      '--app-border-color': 'rgb(90, 85, 80)',
      '--app-hover-color': 'rgb(255, 226, 182)',
    },
  },
  {
    id: Theme.Snow,
    name: 'Snow',
    cssVariables: {
      '--app-bg-primary': 'rgb(248, 249, 250)',
      '--app-bg-secondary': 'rgb(255, 255, 255)',
      '--app-surface': 'rgb(243, 236, 233)',
      '--app-card-active': 'rgb(233, 236, 239)',
      '--app-button-text': 'rgb(255, 255, 255)',
      '--app-text-primary': 'rgb(33, 37, 41)',
      '--app-text-secondary': 'rgb(108, 117, 125)',
      '--app-accent': 'rgb(215, 123, 107)',
      '--app-border-color': 'rgb(222, 226, 230)',
      '--app-hover-color': 'rgb(215, 123, 107)',
    },
  },
  {
    id: Theme.Arctic,
    name: 'Arctic',
    cssVariables: {
      '--app-bg-primary': 'rgb(248, 249, 250)',
      '--app-bg-secondary': 'rgb(255, 255, 255)',
      '--app-surface': 'rgb(240, 245, 249)',
      '--app-card-active': 'rgb(233, 236, 239)',
      '--app-button-text': 'rgb(255, 255, 255)',
      '--app-text-primary': 'rgb(33, 37, 41)',
      '--app-text-secondary': 'rgb(108, 117, 125)',
      '--app-accent': 'rgb(100, 120, 140)',
      '--app-border-color': 'rgb(222, 226, 230)',
      '--app-hover-color': 'rgb(100, 120, 140)',
    },
  },
];

export const DEFAULT_THEME_ID = Theme.Dark;
