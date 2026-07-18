type IconProps = {
  className?: string;
};

const iconClass = (className?: string) => `h-[18px] w-[18px] shrink-0 ${className ?? ''}`;

export function FolderIcon({ open = false, className }: IconProps & { open?: boolean }) {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className={iconClass(className)} fill="none">
      <path
        d={open ? 'M3.5 7.5h6l2-2h3.2l1.8 2H21l-2 10.5H5.2L3.5 7.5Z' : 'M3.5 6.5h6l2-2h3.2l1.8 2H21v11.5H3.5V6.5Z'}
        fill="currentColor"
        opacity="0.9"
      />
      {open ? <path d="M5.2 18 7 10h14l-2 8H5.2Z" fill="currentColor" /> : null}
    </svg>
  );
}

export function FileIcon({ name = '', binary = false, className }: IconProps & { name?: string; binary?: boolean }) {
  const extension = name.toLowerCase().split('.').pop() ?? '';
  const code = ['js', 'jsx', 'ts', 'tsx', 'html', 'css', 'scss', 'json', 'rs', 'py', 'java', 'go', 'sh', 'ps1', 'xml', 'yaml', 'yml'].includes(extension);
  const archive = ['zip', '7z', 'rar', 'tar', 'gz', 'bz2', 'xz'].includes(extension);
  const image = ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'ico', 'bmp'].includes(extension);
  const color = archive ? 'text-violet-500' : image ? 'text-emerald-500' : binary ? 'text-amber-500' : code ? 'text-sky-500' : 'text-slate-500';

  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className={`${iconClass(className)} ${color}`} fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round">
      <path d="M6 2.75h7l5 5V21.25H6V2.75Z" fill="currentColor" fillOpacity="0.08" />
      <path d="M13 2.75v5h5" />
      {code ? <path d="m10 12-2 2 2 2m4-4 2 2-2 2" /> : archive ? <path d="M12 9v1.5m0 1.5v1.5m0 1.5v1.5" /> : image ? <><circle cx="10" cy="11" r="1" /><path d="m8 17 3-3 2 2 1.5-1.5L17 17" /></> : <path d="M9 12h6M9 15h6" />}
    </svg>
  );
}

export function SearchIcon({ className }: IconProps) {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className={iconClass(className)} fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round">
      <circle cx="10.5" cy="10.5" r="5.5" />
      <path d="m15 15 4 4" />
    </svg>
  );
}
