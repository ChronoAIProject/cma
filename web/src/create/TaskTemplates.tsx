import { Button } from '../ui/Button';
import {
  IconCodeBracketsOutline16,
  IconFileOutline16,
  IconGlobeOutline16,
  IconSearchOutline16,
  IconTableOutline16,
} from '../ui/icons';

import { taskTemplates, type TaskTemplateId } from './taskTemplateModel';
const icons = {
  research: <IconSearchOutline16 />,
  spreadsheet: <IconTableOutline16 />,
  document: <IconFileOutline16 />,
  website: <IconGlobeOutline16 />,
  code: <IconCodeBracketsOutline16 />,
};

export function TaskTemplates({ onSelect }: { readonly onSelect: (id: TaskTemplateId) => void }) {
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
