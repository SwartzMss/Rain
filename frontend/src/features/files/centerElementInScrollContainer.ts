export function centerElementInScrollContainer(
  container: HTMLElement,
  element: HTMLElement
): void {
  const containerRect = container.getBoundingClientRect();
  const elementRect = element.getBoundingClientRect();
  const containerCenter =
    containerRect.top + container.clientTop + container.clientHeight / 2;
  const elementCenter = elementRect.top + elementRect.height / 2;

  container.scrollTop += elementCenter - containerCenter;
}
