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
  return next;
}
