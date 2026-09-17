// One icon set, one style: Tabler's outline icons, registered as Shoelace's default
// icon library so every `<sl-icon name="...">` in the app draws from it. The files
// are bundled, so nothing is fetched from a CDN.

import { registerIconLibrary } from '@shoelace-style/shoelace/dist/utilities/icon-library.js';
import activity from '@tabler/icons/outline/activity.svg?url';
import book from '@tabler/icons/outline/book.svg?url';
import chartBar from '@tabler/icons/outline/chart-bar.svg?url';
import chevronDown from '@tabler/icons/outline/chevron-down.svg?url';
import chevronRight from '@tabler/icons/outline/chevron-right.svg?url';
import chevronUp from '@tabler/icons/outline/chevron-up.svg?url';
import clock from '@tabler/icons/outline/clock.svg?url';
import deviceDesktop from '@tabler/icons/outline/device-desktop.svg?url';
import deviceDesktopCog from '@tabler/icons/outline/device-desktop-cog.svg?url';
import exclamationMark from '@tabler/icons/outline/exclamation-mark.svg?url';
import fileText from '@tabler/icons/outline/file-text.svg?url';
import folder from '@tabler/icons/outline/folder.svg?url';
import helpCircle from '@tabler/icons/outline/help-circle.svg?url';
import menu from '@tabler/icons/outline/menu-2.svg?url';
import moon from '@tabler/icons/outline/moon.svg?url';
import pencil from '@tabler/icons/outline/pencil.svg?url';
import plus from '@tabler/icons/outline/plus.svg?url';
import refresh from '@tabler/icons/outline/refresh.svg?url';
import rotate from '@tabler/icons/outline/rotate.svg?url';
import search from '@tabler/icons/outline/search.svg?url';
import selector from '@tabler/icons/outline/selector.svg?url';
import settings from '@tabler/icons/outline/settings.svg?url';
import sun from '@tabler/icons/outline/sun.svg?url';
import terminal from '@tabler/icons/outline/terminal-2.svg?url';
import trash from '@tabler/icons/outline/trash.svg?url';
import world from '@tabler/icons/outline/world.svg?url';
import x from '@tabler/icons/outline/x.svg?url';

const files: Record<string, string> = {
  activity,
  book,
  'chart-bar': chartBar,
  'chevron-down': chevronDown,
  'chevron-right': chevronRight,
  'chevron-up': chevronUp,
  clock,
  'device-desktop': deviceDesktop,
  'device-desktop-cog': deviceDesktopCog,
  'exclamation-mark': exclamationMark,
  'file-text': fileText,
  folder,
  'help-circle': helpCircle,
  menu,
  moon,
  pencil,
  plus,
  refresh,
  rotate,
  search,
  selector,
  settings,
  sun,
  terminal,
  trash,
  world,
  x,
};

export type IconName = keyof typeof files;

export function registerIcons(): void {
  registerIconLibrary('default', {
    resolver: (name) => files[name] ?? '',
    // The set draws at 24 px with a 2 px stroke; at the sizes used here a lighter
    // stroke matches the weight of the text beside it.
    mutator: (svg) => {
      svg.setAttribute('stroke-width', '1.75');
      svg.setAttribute('fill', 'none');
    },
  });
}
