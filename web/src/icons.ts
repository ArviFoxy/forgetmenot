// One icon set, one style: Lucide, registered as Shoelace's default icon library so
// every `<sl-icon name="...">` in the app draws from it. The files are bundled, so
// nothing is fetched from a CDN.

import { registerIconLibrary } from '@shoelace-style/shoelace/dist/utilities/icon-library.js';
import activity from 'lucide-static/icons/activity.svg?url';
import archive from 'lucide-static/icons/archive.svg?url';
import book from 'lucide-static/icons/book.svg?url';
import chartColumn from 'lucide-static/icons/chart-column.svg?url';
import circleAlert from 'lucide-static/icons/circle-alert.svg?url';
import circleHelp from 'lucide-static/icons/circle-help.svg?url';
import clock from 'lucide-static/icons/clock.svg?url';
import fileText from 'lucide-static/icons/file-text.svg?url';
import folder from 'lucide-static/icons/folder.svg?url';
import folderOpen from 'lucide-static/icons/folder-open.svg?url';
import globe from 'lucide-static/icons/globe.svg?url';
import menu from 'lucide-static/icons/menu.svg?url';
import monitor from 'lucide-static/icons/monitor.svg?url';
import monitorCog from 'lucide-static/icons/monitor-cog.svg?url';
import moon from 'lucide-static/icons/moon.svg?url';
import network from 'lucide-static/icons/network.svg?url';
import pencil from 'lucide-static/icons/pencil.svg?url';
import plus from 'lucide-static/icons/plus.svg?url';
import refreshCw from 'lucide-static/icons/refresh-cw.svg?url';
import rotateCcw from 'lucide-static/icons/rotate-ccw.svg?url';
import search from 'lucide-static/icons/search.svg?url';
import sun from 'lucide-static/icons/sun.svg?url';
import terminal from 'lucide-static/icons/terminal.svg?url';
import trash from 'lucide-static/icons/trash-2.svg?url';
import x from 'lucide-static/icons/x.svg?url';

const files: Record<string, string> = {
  activity,
  archive,
  book,
  'chart-column': chartColumn,
  'circle-alert': circleAlert,
  'circle-help': circleHelp,
  clock,
  'file-text': fileText,
  folder,
  'folder-open': folderOpen,
  globe,
  menu,
  monitor,
  'monitor-cog': monitorCog,
  moon,
  network,
  pencil,
  plus,
  'refresh-cw': refreshCw,
  'rotate-ccw': rotateCcw,
  search,
  sun,
  terminal,
  'trash-2': trash,
  x,
};

export type IconName = keyof typeof files;

export function registerIcons(): void {
  registerIconLibrary('default', {
    resolver: (name) => files[name] ?? '',
    // Lucide draws at 24px with a 2px stroke; at the sizes used here a lighter
    // stroke matches the text weight.
    mutator: (svg) => {
      svg.setAttribute('stroke-width', '1.75');
      svg.setAttribute('fill', 'none');
    },
  });
}
