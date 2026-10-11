export type OnboardingProgress = {
  enabled: boolean;
  introDismissed: boolean;
  completed: string[];
};

const defaults: OnboardingProgress = {
  enabled: true,
  introDismissed: false,
  completed: []
};

export const onboardingStorageKey = (identity: string) => `rain:onboarding:v1:${identity}`;
const onboardingProgressChangedEvent = 'rain:onboarding-progress-changed';

type OnboardingProgressChange = {
  identity: string;
  progress: OnboardingProgress;
};

export function readOnboardingProgress(identity: string): OnboardingProgress {
  try {
    const value = localStorage.getItem(onboardingStorageKey(identity));
    if (!value) return defaults;
    const saved = JSON.parse(value) as Partial<OnboardingProgress>;
    return {
      enabled: saved.enabled !== false,
      introDismissed: Boolean(saved.introDismissed),
      completed: Array.isArray(saved.completed) ? saved.completed : []
    };
  } catch {
    return defaults;
  }
}

export function updateOnboardingProgress(identity: string, patch: Partial<OnboardingProgress>) {
  const next = { ...readOnboardingProgress(identity), ...patch };
  try {
    localStorage.setItem(onboardingStorageKey(identity), JSON.stringify(next));
  } catch {
    // Onboarding preferences are optional and must not affect app behavior.
  }
  if (typeof window !== 'undefined') {
    window.dispatchEvent(new CustomEvent<OnboardingProgressChange>(onboardingProgressChangedEvent, {
      detail: { identity, progress: next }
    }));
  }
  return next;
}

export function subscribeOnboardingProgress(
  identity: string,
  onChange: (progress: OnboardingProgress) => void
) {
  if (typeof window === 'undefined') return () => undefined;
  const key = onboardingStorageKey(identity);
  const handleCustomChange = (event: Event) => {
    const detail = (event as CustomEvent<OnboardingProgressChange>).detail;
    if (detail?.identity === identity) onChange(detail.progress);
  };
  const handleStorageChange = (event: StorageEvent) => {
    if (event.key === key || event.key === null) onChange(readOnboardingProgress(identity));
  };
  window.addEventListener(onboardingProgressChangedEvent, handleCustomChange);
  window.addEventListener('storage', handleStorageChange);
  return () => {
    window.removeEventListener(onboardingProgressChangedEvent, handleCustomChange);
    window.removeEventListener('storage', handleStorageChange);
  };
}
