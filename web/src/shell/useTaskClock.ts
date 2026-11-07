import { useEffect, useState } from 'react';
import { nextTaskTick } from './taskGroups';

export function useTaskClock(starts: readonly string[]) {
  const [now, setNow] = useState(() => new Date());
  const key = starts.join('|');
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const refresh = () => {
      clearTimeout(timer);
      const at = Date.now();
      setNow(new Date(at));
      timer = setTimeout(refresh, nextTaskTick(key.split('|').map(Date.parse), at));
    };
    refresh();
    window.addEventListener('focus', refresh);
    document.addEventListener('visibilitychange', refresh);
    return () => {
      clearTimeout(timer);
      window.removeEventListener('focus', refresh);
      document.removeEventListener('visibilitychange', refresh);
    };
  }, [key]);
  return now;
}
