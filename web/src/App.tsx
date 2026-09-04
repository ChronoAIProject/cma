import { Console } from './Console';
import { useAuth } from './auth/context';
import lockup from './assets/brand/cma-lockup-horizontal.svg';
import { useAppRouter } from './routing/routes';
import { Button } from './ui/Button';
import './styles/tokens.css';
import './styles/design-platform.css';
import './styles/effects-typography.css';
import './styles/scrollbar.css';
import './styles/shiki.css';
import './styles/base.css';
import './styles/components.css';
import './AppError.css';

export function App() {
  const { state, signIn, signOut } = useAuth();
  const router = useAppRouter();

  if (state.status === 'checking') {
    return <div className="app-checking" aria-busy="true" />;
  }
  if (state.status === 'error') {
    return (
      <main className="app-error" role="alert">
        <p className="app-error__what">{state.message}</p>
        <p className="app-error__next">Reload the page to try again.</p>
      </main>
    );
  }
  if (state.status === 'anonymous') {
    return (
      <main className="auth-gate">
        <img className="auth-gate__brand" src={lockup} alt="CMA" />
        <Button variant="primary" onClick={() => void signIn(window.location.pathname)}>
          Sign in
        </Button>
      </main>
    );
  }
  return (
    <Console
      user={state.user}
      route={router.route}
      onSignOut={() => {
        void signOut();
      }}
    />
  );
}
