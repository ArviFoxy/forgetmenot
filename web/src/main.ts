import 'modern-normalize/modern-normalize.css';
import '@fontsource-variable/inter/wght.css';
import '@fontsource/jetbrains-mono/latin-400.css';
import './shoelace';
import '@milkdown/crepe/theme/common/style.css';
import '@milkdown/crepe/theme/frame.css';
import './styles/app.css';
import { registerIcons } from './icons';
import './components/fmn-app';
import { interceptLinkClicks } from './navigation';
import { applyScheme, storedScheme, watchScheme } from './theme';

registerIcons();
applyScheme(storedScheme());
watchScheme();
interceptLinkClicks();
