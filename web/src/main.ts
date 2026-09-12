import '@picocss/pico/css/pico.css';
import 'bootstrap-icons/font/bootstrap-icons.css';
import './styles/app.css';
import './components/fmn-app';
import { interceptLinkClicks } from './navigation';
import { applyScheme, storedScheme } from './theme';

applyScheme(storedScheme());
interceptLinkClicks();
