# Skill Review Feedback Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give users explicit, unambiguous feedback while a Skill quality assessment is running, without changing public APIs or backend behavior.

**Architecture:** Track the ID of the Skill currently being reviewed separately from the page's generic mutation state. Route assessment through a dedicated async handler, and let `SkillReviewPanel` replace either an absent or existing score with one loading presentation while that ID is active.

**Tech Stack:** React 18, TypeScript, Tailwind CSS, Vitest, Testing Library

---

### Task 1: Specify the assessment loading behavior

**Files:**
- Test: `frontend/tests/skills-page.behavior.test.tsx`

- [ ] **Step 1: Add a controllable promise helper and review fixture**

Add a `deferred<T>()` helper that exposes `resolve` and `reject`, plus a complete `SkillReview` fixture with an old score. These make the request remain pending long enough to assert the intermediate UI.

```tsx
function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}
```

- [ ] **Step 2: Write failing first-assessment test**

Render a Skill with `review: null`, leave `reviewSkill` pending, click `质量评估`, then assert:

```tsx
expect(screen.getByRole('button', { name: 'AI 评估中...' })).toBeDisabled();
expect(screen.getByRole('status')).toHaveTextContent('正在评估，请稍候…');
expect(screen.queryByText('当前版本尚未评估。')).not.toBeInTheDocument();
```

- [ ] **Step 3: Write failing re-assessment and failure tests**

For an existing review, assert the pending UI hides the old score. Reject the request and assert the existing score returns while the current `role="alert"` contains the normalized error.

```tsx
expect(screen.queryByText('86')).not.toBeInTheDocument();
pending.reject(new Error('AI 服务暂时不可用'));
expect(await screen.findByRole('alert')).toHaveTextContent('AI 服务暂时不可用');
expect(screen.getByText('86')).toBeInTheDocument();
```

- [ ] **Step 4: Run the focused tests and verify RED**

Run from `frontend/`: `npx vitest run tests/skills-page.behavior.test.tsx`

Expected: FAIL because the button still reads `质量评估`, the panel still shows the old/empty review, and there is no assessment status.

- [ ] **Step 5: Commit the failing behavior tests**

```bash
git add frontend/tests/skills-page.behavior.test.tsx
git commit -m "Test skill assessment loading feedback"
```

### Task 2: Implement explicit assessment feedback

**Files:**
- Modify: `frontend/src/features/skills/SkillsPage.tsx`
- Modify: `frontend/src/features/skills/SkillReviewPanel.tsx`

- [ ] **Step 1: Add Skill-specific assessment state and handler**

In `SkillsPage`, replace use of generic `mutate` for assessment with `reviewSkill`:

```tsx
const [reviewingSkillId, setReviewingSkillId] = useState<string | null>(null);

const reviewSkill = async (skillId: string) => {
  setReviewingSkillId(skillId);
  setError('');
  try {
    await rainApi.reviewSkill(skillId);
    await load();
    setDetailRevision((value) => value + 1);
  } catch (reason) {
    setError(normalizeApiError(reason));
  } finally {
    setReviewingSkillId(null);
  }
};
```

- [ ] **Step 2: Render the loading button and connect the panel**

Derive whether the selected Skill is the active assessment, disable assessment globally while one request is active, and render an accessible animated indicator:

```tsx
const reviewingSelectedSkill = reviewingSkillId === selectedSkill.id;

<button disabled={reviewingSkillId !== null || aiProviderConfigured !== true}>
  {reviewingSelectedSkill ? <><span aria-hidden="true" className="inline-block h-4 w-4 animate-spin rounded-full border-2 border-white/40 border-t-white" />AI 评估中...</> : '质量评估'}
</button>
<SkillReviewPanel review={selectedSkill.review} reviewing={reviewingSelectedSkill} />
```

- [ ] **Step 3: Replace the score panel while assessment is pending**

Extend `SkillReviewPanel` with `reviewing?: boolean` and return a live status before inspecting `review`:

```tsx
if (reviewing) {
  return <div role="status" aria-live="polite" className="rounded-xl border border-cyan-100 bg-cyan-50/50 p-4 text-sm text-cyan-800">正在评估，请稍候…</div>;
}
```

- [ ] **Step 4: Run the focused tests and verify GREEN**

Run from `frontend/`: `npx vitest run tests/skills-page.behavior.test.tsx`

Expected: PASS for all tests in `skills-page.behavior.test.tsx`.

- [ ] **Step 5: Commit the implementation**

```bash
git add frontend/src/features/skills/SkillsPage.tsx frontend/src/features/skills/SkillReviewPanel.tsx
git commit -m "Improve skill assessment loading feedback"
```

### Task 3: Verify and publish

**Files:**
- Verify: `frontend/tests/skills-page.behavior.test.tsx`
- Verify: `frontend/src/features/skills/SkillsPage.tsx`
- Verify: `frontend/src/features/skills/SkillReviewPanel.tsx`

- [ ] **Step 1: Run the complete frontend verification suite**

Run from `frontend/`:

```bash
npm test
npm run lint
npm run build
```

Expected: every command exits 0 with no test failures or TypeScript errors.

- [ ] **Step 2: Inspect the final patch**

```bash
git diff origin/main...HEAD
git diff --check origin/main...HEAD
git status --short
```

Expected: only the design, plan, tests, and two Skill UI files are changed; whitespace check passes; worktree is clean.

- [ ] **Step 3: Push and create the Draft PR**

Push `agent/issue-102-review-feedback`, then create a Draft PR targeting `main` whose body summarizes the new feedback, lists verification commands, and includes `Closes #102`.
