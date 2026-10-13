// Outline icons in the style of Fluent System Icons (20×20, 1.5px strokes), drawn for deej.

import type { SVGProps } from "react";

type IconProps = SVGProps<SVGSVGElement> & { size?: number };

function Icon({ size = 20, children, ...props }: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      {...props}
    >
      {children}
    </svg>
  );
}

export const SpeakerIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M3 7.5h2.5L9.5 4v12l-4-3.5H3z" />
    <path d="M13 7a4 4 0 0 1 0 6M15.5 4.5a7.5 7.5 0 0 1 0 11" />
  </Icon>
);

export const MicIcon = (p: IconProps) => (
  <Icon {...p}>
    <rect x="7" y="2.5" width="6" height="10" rx="3" />
    <path d="M4.5 10a5.5 5.5 0 0 0 11 0M10 15.5v2" />
  </Icon>
);

export const MixerIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M5 3v14M10 3v14M15 3v14" />
    <rect x="3" y="11" width="4" height="2.5" rx="1" fill="currentColor" stroke="none" />
    <rect x="8" y="6" width="4" height="2.5" rx="1" fill="currentColor" stroke="none" />
    <rect x="13" y="9" width="4" height="2.5" rx="1" fill="currentColor" stroke="none" />
  </Icon>
);

export const BoardIcon = (p: IconProps) => (
  <Icon {...p}>
    <rect x="3" y="5" width="14" height="10" rx="2" />
    <path d="M6.5 8v4M10 8v4M13.5 8v4M8 2.5v2.5M12 2.5v2.5" />
  </Icon>
);

export const LinkIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M8.5 11.5a3 3 0 0 0 4.2 0l2.6-2.6a3 3 0 0 0-4.2-4.2L10 5.8" />
    <path d="M11.5 8.5a3 3 0 0 0-4.2 0l-2.6 2.6a3 3 0 0 0 4.2 4.2l1.1-1.1" />
  </Icon>
);

export const SettingsIcon = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="10" cy="10" r="2.5" />
    <path d="M10 2.5l1.3 1.9 2.2-.6.6 2.2 1.9 1.3-1 2.1 1 2.1-1.9 1.3-.6 2.2-2.2-.6L10 17.5l-1.3-1.9-2.2.6-.6-2.2-1.9-1.3 1-2.1-1-2.1 1.9-1.3.6-2.2 2.2.6z" />
  </Icon>
);

export const SearchIcon = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="8.5" cy="8.5" r="5" />
    <path d="M12.5 12.5l4.5 4.5" />
  </Icon>
);

export const AddIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M10 4v12M4 10h12" />
  </Icon>
);

export const DismissIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M5 5l10 10M15 5L5 15" />
  </Icon>
);

export const RefreshIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M15.5 10a5.5 5.5 0 1 1-1.6-3.9" />
    <path d="M14.5 2.5v4h-4" />
  </Icon>
);

export const ChevronDownIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M5.5 8l4.5 4.5L14.5 8" />
  </Icon>
);

export const DocumentIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M5 2.5h6.5l3.5 3.5v11.5H5z" />
    <path d="M11.5 2.5V6H15M7.5 10h5M7.5 13h5" />
  </Icon>
);

export const FolderIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M2.5 5.5a1.5 1.5 0 0 1 1.5-1.5h3.5l2 2H16a1.5 1.5 0 0 1 1.5 1.5v7A1.5 1.5 0 0 1 16 16H4a1.5 1.5 0 0 1-1.5-1.5z" />
  </Icon>
);

export const CheckIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M4.5 10.5l3.5 3.5 7.5-8" />
  </Icon>
);

export const WarningIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M10 3l7.5 13.5h-15z" />
    <path d="M10 8v4M10 14.5v.1" />
  </Icon>
);

export const InfoIcon = (p: IconProps) => (
  <Icon {...p}>
    <circle cx="10" cy="10" r="7.5" />
    <path d="M10 9v5M10 6.5v.1" />
  </Icon>
);

export const AppIcon = (p: IconProps) => (
  <Icon {...p}>
    <rect x="3" y="4" width="14" height="12" rx="2" />
    <path d="M3 7.5h14" />
  </Icon>
);

export const FocusIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M3 7V4.5A1.5 1.5 0 0 1 4.5 3H7M13 3h2.5A1.5 1.5 0 0 1 17 4.5V7M17 13v2.5a1.5 1.5 0 0 1-1.5 1.5H13M7 17H4.5A1.5 1.5 0 0 1 3 15.5V13" />
    <circle cx="10" cy="10" r="2.5" />
  </Icon>
);

export const StackIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M10 3l7 3.5-7 3.5-7-3.5z" />
    <path d="M3 10l7 3.5 7-3.5M3 13.5L10 17l7-3.5" />
  </Icon>
);

export const BellIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M5 13.5V9a5 5 0 0 1 10 0v4.5l1.5 2h-13z" />
    <path d="M8.5 17.5a1.5 1.5 0 0 0 3 0" />
  </Icon>
);

export const MonitorIcon = (p: IconProps) => (
  <Icon {...p}>
    <rect x="2.5" y="3.5" width="15" height="10" rx="1.5" />
    <path d="M7 17h6M10 13.5V17" />
  </Icon>
);

export const PowerIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M10 2.5v7" />
    <path d="M6 5a6 6 0 1 0 8 0" />
  </Icon>
);

export const TrayIcon = (p: IconProps) => (
  <Icon {...p}>
    <path d="M2.5 11.5h4l1.5 2h4l1.5-2h4" />
    <path d="M4.5 5.5h11l2 6v3.5a1.5 1.5 0 0 1-1.5 1.5H4a1.5 1.5 0 0 1-1.5-1.5v-3.5z" />
  </Icon>
);
