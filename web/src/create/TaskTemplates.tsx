import { Button } from '../ui/Button';
import { useState } from 'react';
import { Menu } from '../ui/Menu';
import {
  IconCodeBracketsOutline16,
  IconFileOutline16,
  IconGlobeOutline16,
  IconSearchOutline16,
  IconTableOutline16,
  IconChevronDownOutline14,
} from '../ui/icons';

import { taskTemplates, type TaskTemplateId } from './taskTemplateModel';
const icons = {
  research: <IconSearchOutline16 />,
  spreadsheet: <IconTableOutline16 />,
  document: <IconFileOutline16 />,
  website: <IconGlobeOutline16 />,
  code: <IconCodeBracketsOutline16 />,
};

export function TaskTemplates({
  onSelect,
  presentation = 'expanded',
}: {
  readonly onSelect: (id: TaskTemplateId) => void;
  readonly presentation?: 'expanded' | 'menu';
}) {
  const [open, setOpen] = useState(false);
  if (presentation === 'menu')
    return (
      <Menu
        open={open}
        label="Task templates"
        onClose={() => setOpen(false)}
        anchor={
          <Button
            placement="task-home__tool"
            aria-haspopup="menu"
            aria-expanded={open}
            onClick={() => setOpen(!open)}
          >
            Templates <IconChevronDownOutline14 />
          </Button>
        }
        items={taskTemplates.map(({ id, name }) => ({ id, label: name, icon: icons[id] }))}
        onSelect={(id) => {
          const selected = taskTemplates.find((template) => template.id === id);
          if (selected) onSelect(selected.id);
          setOpen(false);
        }}
      />
    );
  return (
    <div className="task-home__templates" role="group" aria-label="Task templates">
      {taskTemplates.map(({ id, name }) => (
        <Button
          key={name}
          placement="task-home__template"
          variant="outline"
          icon={icons[id]}
          onClick={() => onSelect(id)}
        >
          {name}
        </Button>
      ))}
    </div>
  );
}
