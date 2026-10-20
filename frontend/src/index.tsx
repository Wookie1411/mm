import { faPuzzlePiece } from '@fortawesome/free-solid-svg-icons';
import { FontAwesomeIcon } from '@fortawesome/react-fontawesome';
import { Extension, ExtensionContext } from 'shared';
import AdminSettings from './AdminSettings.tsx';
import ModManager from './ModManager.tsx';
import SidebarFilter from './SidebarFilter.tsx';

class DevEmrekmekModmanagerExtension extends Extension {
  public cardConfigurationPage: React.FC | null = AdminSettings;
  public cardComponent: React.FC | null = null;
  public cardIcon: React.ReactNode = <FontAwesomeIcon icon={faPuzzlePiece} />;

  public initialize(ctx: ExtensionContext): void {
    ctx.extensionRegistry.enterRoutes((routes) =>
      routes.addServerRoute({
        name: 'Mods',
        icon: faPuzzlePiece,
        path: '/mod-manager',
        element: ModManager,
        permission: ['mod-manager.read', 'mod-manager.manage', 'files.read-content'],
      }),
    );
    ctx.extensionRegistry.pages.server.prependComponent(SidebarFilter);
    ctx.extensionRegistry.permissionIcons.addServerPermissionIcon(
      'mod-manager',
      <FontAwesomeIcon icon={faPuzzlePiece} />,
    );
  }
}

export default new DevEmrekmekModmanagerExtension();
