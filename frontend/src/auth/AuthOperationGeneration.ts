export class AuthOperationGeneration {
  private generation = 0;
  private activeMutations = 0;

  begin(): number {
    this.generation += 1;
    return this.generation;
  }

  invalidate(): void {
    this.generation += 1;
  }

  beginMutation(): () => void {
    this.activeMutations += 1;
    this.invalidate();
    let finished = false;
    return () => {
      if (finished) return;
      finished = true;
      this.activeMutations -= 1;
      this.invalidate();
    };
  }

  isCurrent(generation: number): boolean {
    return this.activeMutations === 0 && generation === this.generation;
  }
}
