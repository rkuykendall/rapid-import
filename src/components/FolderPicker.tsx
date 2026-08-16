import { open } from '@tauri-apps/plugin-dialog';
import { FolderOpen } from 'lucide-react';
import Button from './ui/Button';
import Input from './ui/Input';
import Text from './ui/Text';
import { TextVariants } from '../types/typography';

interface FolderPickerProps {
  label: string;
  value: string;
  onChange(path: string): void;
}

export default function FolderPicker({ label, value, onChange }: FolderPickerProps) {
  const handleBrowse = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === 'string') {
      onChange(selected);
    }
  };

  return (
    <div className="flex flex-col gap-1">
      <Text variant={TextVariants.label}>{label}</Text>
      <div className="flex gap-2">
        <Input value={value} onChange={(e) => onChange(e.target.value)} placeholder="/path/to/folder" />
        <Button onClick={handleBrowse} className="bg-surface shrink-0" title={`Browse for ${label}`}>
          <FolderOpen size={16} />
        </Button>
      </div>
    </div>
  );
}
