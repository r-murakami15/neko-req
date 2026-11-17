import type { SVGProps } from "react";

function Svg({ children, ...props }: SVGProps<SVGSVGElement>) {
  return (
    <svg
      aria-hidden="true"
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      {...props}
    >
      {children}
    </svg>
  );
}

export const PencilIcon = () => (
  <Svg>
    <path d="M4 20h4L19 9l-4-4L4 16z" />
    <path d="M13 7l4 4" />
  </Svg>
);

export const DownloadIcon = () => (
  <Svg>
    <path d="M12 3v12" />
    <path d="M7 10l5 5 5-5" />
    <path d="M5 21h14" />
  </Svg>
);

export const GearIcon = () => (
  <Svg>
    <circle cx="12" cy="12" r="3" />
    <path d="M12 2v3M12 19v3M2 12h3M19 12h3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M4.9 19.1L7 17M17 7l2.1-2.1" />
  </Svg>
);

export const SearchIcon = () => (
  <Svg width="14" height="14" strokeWidth="2">
    <circle cx="11" cy="11" r="7" />
    <path d="M20 20l-3.5-3.5" />
  </Svg>
);

export const ListIcon = () => (
  <Svg width="15" height="15">
    <path d="M4 7h16M4 12h16M4 17h10" />
  </Svg>
);

export const ClockIcon = () => (
  <Svg width="15" height="15">
    <circle cx="12" cy="12" r="8" />
    <path d="M12 8v4l3 2" />
  </Svg>
);

export const ArrowRightIcon = () => (
  <Svg width="14" height="14" strokeWidth="2.2">
    <path d="M5 12h14" />
    <path d="M13 6l6 6-6 6" />
  </Svg>
);

export const InfoIcon = () => (
  <Svg width="14" height="14" strokeWidth="2">
    <circle cx="12" cy="12" r="9" />
    <path d="M12 8v5M12 16h.01" />
  </Svg>
);

export const ChevronDownIcon = () => (
  <Svg width="12" height="12" strokeWidth="2.2">
    <path d="M6 9l6 6 6-6" />
  </Svg>
);

export const CheckIcon = () => (
  <Svg width="13" height="13" strokeWidth="2.4">
    <path d="M5 12l5 5 9-10" />
  </Svg>
);

/** アプリのロゴ（猫の顔）。 */
export function CatLogo({ size = 28 }: { size?: number }) {
  return (
    <span className="logo" aria-hidden="true" style={{ width: size, height: size }}>
      <svg width={size * 0.7} height={size * 0.7} viewBox="0 0 24 24" fill="currentColor">
        <path d="M3 10.5 4.2 3.4 9.3 6.8Q12 6.3 14.7 6.8L19.8 3.4 21 10.5Q21.6 18 12 19.6 2.4 18 3 10.5Z" />
        <circle cx="8.8" cy="11.6" r="1.2" fill="var(--accent)" />
        <circle cx="15.2" cy="11.6" r="1.2" fill="var(--accent)" />
      </svg>
    </span>
  );
}

export const CloseIcon = () => (
  <Svg width="12" height="12" strokeWidth="2.2">
    <path d="M6 6l12 12M18 6L6 18" />
  </Svg>
);

export const EyeIcon = () => (
  <Svg>
    <path d="M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12z" />
    <circle cx="12" cy="12" r="3" />
  </Svg>
);

export const EyeOffIcon = () => (
  <Svg>
    <path d="M3 3l18 18" />
    <path d="M10.6 5.1A10 10 0 0 1 12 5c6.4 0 10 7 10 7a17 17 0 0 1-3.2 4M6.4 6.5C3.6 8.3 2 12 2 12s3.6 7 10 7c1.6 0 3-.4 4.2-1" />
    <path d="M9.9 9.9a3 3 0 0 0 4.2 4.2" />
  </Svg>
);

export const LockIcon = () => (
  <Svg width="14" height="14">
    <rect x="5" y="11" width="14" height="10" rx="2" />
    <path d="M8 11V8a4 4 0 0 1 8 0v3" />
  </Svg>
);

export const ChevronLeftIcon = () => (
  <Svg strokeWidth="2">
    <path d="M15 6l-6 6 6 6" />
  </Svg>
);

export const FolderIcon = () => (
  <Svg width="15" height="15">
    <path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
  </Svg>
);

export const TrashIcon = () => (
  <Svg width="14" height="14">
    <path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3" />
  </Svg>
);
